//! Avid projects: find them below a folder, read their bins and list the media files their clips need that are
//! not on the connected drives; write those as a request for MediaFiles Ferry.
use crate::avb::{Bin, MobId, MobKind, id_hex};
use crate::{PMR, folders, key, read_pmr_entries};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn visible_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !name_of(p).starts_with('.')).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn has_avp(dir: &Path) -> bool {
    fs::read_dir(dir).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".avp"))).unwrap_or(false)
}

/// Avid projects below a folder: folders with an .avp file. Inside a project no further projects are looked for
/// (shared projects keep more .avp files in AvidSharedData/<user>).
pub fn find_projects(root: &Path) -> Vec<PathBuf> {
    if has_avp(root) {
        return vec![root.to_path_buf()];
    }
    visible_dirs(root).iter().flat_map(|d| find_projects(d)).collect()
}

/// Bins of a project, without Avid's backups in "Attic".
pub fn bins(project: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![project.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let (p, n) = (e.path(), e.file_name().to_string_lossy().into_owned());
            if n.starts_with('.') || n.eq_ignore_ascii_case("Attic") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if n.to_lowercase().ends_with(".avb") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Media files of one drive by mob ID: from the Avid databases, counted only if the file is really in the folder;
/// MXF files that are in no database are identified by their header. Returns (ids, read from a header,
/// unidentified files). `tick` is called per folder.
fn drive_media(drive: &Path, tick: &dyn Fn()) -> (HashSet<MobId>, usize, usize) {
    let mut ids = HashSet::new();
    let (mut from_header, mut unknown) = (0, 0);
    for (_, dir) in folders(drive) {
        tick();
        let on_disk: Vec<String> = fs::read_dir(&dir)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| key(n).ends_with(".mxf") && !n.starts_with('.')).collect())
            .unwrap_or_default();
        let entries = fs::read(dir.join(PMR)).ok().and_then(|d| read_pmr_entries(&d).ok()).unwrap_or_default();
        let in_db: HashSet<String> = entries.iter().map(|e| key(&e.name)).collect();
        let present: HashSet<String> = on_disk.iter().map(|n| key(n)).collect();
        ids.extend(entries.iter().filter(|e| present.contains(&key(&e.name))).map(|e| e.id));
        for n in on_disk.iter().filter(|n| !in_db.contains(&key(n))) {
            match crate::mxf::file_mob_id(&dir.join(n)) {
                Ok(id) => {
                    ids.insert(id);
                    from_header += 1;
                }
                Err(_) => unknown += 1,
            }
        }
    }
    (ids, from_header, unknown)
}

pub struct Available {
    pub ids: HashSet<MobId>,
    /// MXF files in no Avid database, identified by their header
    pub from_header: usize,
    /// MXF files in no Avid database whose header could not be read
    pub unknown: usize,
    /// drives that stopped answering and were skipped
    pub skipped: Vec<PathBuf>,
}

/// Media files on the given drives. A drive that does not answer for `patience` (a hanging network or macFUSE
/// mount blocks a read forever) is skipped instead of stopping the scan; its reading thread is left behind.
pub fn available_media(drives: &[PathBuf], patience: std::time::Duration) -> Available {
    use std::sync::mpsc;
    let mut a = Available { ids: HashSet::new(), from_header: 0, unknown: 0, skipped: vec![] };
    for d in drives {
        let (tx, rx) = mpsc::channel();
        let drive = d.clone();
        std::thread::spawn(move || {
            let tick = tx.clone();
            let result = drive_media(&drive, &move || {
                let _ = tick.send(None);
            });
            let _ = tx.send(Some(result));
        });
        loop {
            match rx.recv_timeout(patience) {
                Ok(None) => continue,
                Ok(Some((ids, from_header, unknown))) => {
                    a.ids.extend(ids);
                    a.from_header += from_header;
                    a.unknown += unknown;
                    break;
                }
                Err(_) => {
                    a.skipped.push(d.clone());
                    break;
                }
            }
        }
    }
    a
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ClipKind {
    Master,
    Subclip,
    Sequence,
}

/// Where a missing media file is used.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Use {
    pub project: String,
    /// bin path relative to the project
    pub bin: String,
    pub clip: String,
    pub kind: ClipKind,
}

#[derive(Debug, Clone)]
pub struct Missing {
    pub id: MobId,
    /// the master clip the file belongs to, else the file's name in the bin, else the first clip that uses it
    pub name: String,
    /// the media file has no Avid media reference (MSML) – e.g. AAF import
    pub unmanaged: bool,
    /// another media file of the same source (camera clip / tape, picture or sound) is on the drives:
    /// the clip only lost its link, relinking in Media Composer finds it – not requested
    pub relink: bool,
    /// first clip found that uses it
    pub first: Use,
}

/// Missing media files of one bin with the same name (e.g. the video and audio files of one clip).
#[derive(Debug, Clone, PartialEq)]
pub struct MissingGroup {
    pub name: String,
    pub files: usize,
    /// all of them can be relinked (see `Missing::relink`)
    pub relink: bool,
    /// where in the bin they are used
    pub uses: Vec<(String, ClipKind)>,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub projects: usize,
    pub bins: usize,
    /// bins that could not be read: (path, reason)
    pub failed: Vec<(PathBuf, String)>,
    pub clips: usize,
    /// every missing media file once (without the relinkable ones), sorted by its first use – the request
    pub missing: Vec<Missing>,
    /// missing files whose source is on the drives in another media file (relink in Media Composer)
    pub relink: usize,
    /// AMA-linked files – left out of the request
    pub ama: usize,
    /// (project, bin) -> the AMA-linked files its clips use, grouped by name (`relink` always false)
    pub ama_by_bin: BTreeMap<(String, String), Vec<MissingGroup>>,
    /// AMA-linked files whose master clip has a namesake in the project with all its media on the drives
    /// (e.g. linked first, imported later) – they don't matter, only counted
    pub ama_covered: usize,
    /// file mobs without any locator: no media was ever made for them (e.g. the camera sound tracks of a master
    /// clip whose sound was never imported – Media Composer still shows the clip online) – left out
    pub no_media: usize,
    /// (project, bin) -> its missing media files, grouped by name
    pub by_bin: BTreeMap<(String, String), Vec<MissingGroup>>,
    /// missing render files (precomputed effects) – not requested, Media Composer renders them again
    pub renders: usize,
}

fn kind_of(kind: MobKind, usage: i32) -> Option<ClipKind> {
    match (kind, usage) {
        (MobKind::Master, 2) | (MobKind::Composition, 2) => Some(ClipKind::Subclip),
        // usage 1: master clip of a render file (bins keep them hidden)
        (MobKind::Master, 1) => None,
        (MobKind::Master, _) => Some(ClipKind::Master),
        (MobKind::Composition, 0) => Some(ClipKind::Sequence),
        _ => None,
    }
}

/// What one bin contributes to `Refs`: with `uses`, the media and renders of its clips, subclips and sequences (mobs
/// from other bins go to `waiting`); always its clips' media into `media_of`, for clips used from other bins.
fn use_bin(bin: &Bin, r: &mut Refs, media_of: &mut HashMap<MobId, Vec<MobId>>, waiting: &mut Vec<(MobId, bool)>, uses: bool) {
    // only Avid-managed media (AMA-linked and other linked files are not in Avid MediaFiles)
    let managed = |ids: Vec<MobId>| -> Vec<MobId> { ids.into_iter().filter(|id| bin.mob(id).is_none_or(|m| !m.ama && m.locator.as_deref().is_none_or(|l| l == "MSML"))).collect() };
    for &item in &bin.items {
        let mob = &bin.mobs[item];
        let (mut gone, mut gone_renders) = (HashSet::new(), HashSet::new());
        let media = managed(bin.media_of(item, &mut gone));
        if mob.kind != MobKind::Composition {
            media_of.insert(mob.id, media.clone());
        }
        // a render's own master clip is no use of it – only an effect that still points to it is
        if !uses || kind_of(mob.kind, mob.usage).is_none() {
            continue;
        }
        r.renders.extend(managed(bin.renders_of(item, &mut gone_renders)));
        r.media.extend(media);
        waiting.extend(gone.into_iter().map(|id| (id, false)));
        waiting.extend(gone_renders.into_iter().map(|id| (id, true)));
    }
}

/// Media and renders one bin uses; clips it takes from other bins of its project are looked up there.
pub fn bin_refs(path: &Path) -> Result<Refs, String> {
    let bin = Bin::read(path)?;
    let mut r = Refs { bins: 1, ..Default::default() };
    let (mut media_of, mut waiting) = (HashMap::new(), vec![]);
    use_bin(&bin, &mut r, &mut media_of, &mut waiting, true);
    if !waiting.is_empty()
        && let Some(project) = path.ancestors().skip(1).find(|d| has_avp(d))
    {
        for other in bins(project).iter().filter(|b| b.as_path() != path) {
            match Bin::read(other) {
                Ok(b) => use_bin(&b, &mut Refs::default(), &mut media_of, &mut vec![], false),
                Err(e) => r.failed.push((other.clone(), e)),
            }
        }
    }
    for (mob, render) in waiting {
        let set = if render { &mut r.renders } else { &mut r.media };
        set.extend(media_of.get(&mob).into_iter().flatten());
    }
    Ok(r)
}

/// The media files the bins of some projects use: `media` through their clips, subclips and sequences, `renders`
/// through the effects of those (a render whose effect is gone from every sequence is in neither).
#[derive(Debug, Default)]
pub struct Refs {
    pub media: HashSet<MobId>,
    pub renders: HashSet<MobId>,
    pub bins: usize,
    /// bins that could not be read: (bin, error) – their media count as unused, so the caller must say so
    pub failed: Vec<(PathBuf, String)>,
}

/// Reads every bin of the projects (see `Refs`). Clips stored in another bin of the same project are resolved over
/// the project. progress gets (bins done, bins total, bin).
pub fn references(projects: &[PathBuf], progress: &mut dyn FnMut(usize, usize, &str)) -> Refs {
    let all: Vec<(usize, PathBuf)> = projects.iter().enumerate().flat_map(|(i, p)| bins(p).into_iter().map(move |b| (i, b))).collect();
    let mut r = Refs { bins: all.len(), ..Default::default() };
    // per project: mob -> its media files; mobs from another bin, waiting for the end of the project (render?)
    let mut media_of: HashMap<MobId, Vec<MobId>> = HashMap::new();
    let mut waiting: Vec<(MobId, bool)> = vec![];
    let finish = |media_of: &mut HashMap<MobId, Vec<MobId>>, waiting: &mut Vec<(MobId, bool)>, r: &mut Refs| {
        for (mob, render) in waiting.drain(..) {
            let set = if render { &mut r.renders } else { &mut r.media };
            set.extend(media_of.get(&mob).into_iter().flatten());
        }
        media_of.clear();
    };
    let mut current = usize::MAX;
    for (i, (pi, path)) in all.iter().enumerate() {
        if *pi != current {
            finish(&mut media_of, &mut waiting, &mut r);
            current = *pi;
        }
        progress(i, all.len(), &format!("{}/{}", name_of(&projects[*pi]), name_of(path)));
        let bin = match Bin::read(path) {
            Ok(b) => b,
            Err(e) => {
                r.failed.push((path.clone(), e));
                continue;
            }
        };
        use_bin(&bin, &mut r, &mut media_of, &mut waiting, true);
    }
    finish(&mut media_of, &mut waiting, &mut r);
    r
}

/// Reads every bin of the projects and collects the media files their clips, subclips and sequences need that are
/// not in `available`. References to clips stored in another bin of the same project are resolved over the project.
/// progress gets (bins done, bins total, bin).
pub fn scan(projects: &[PathBuf], available: &HashSet<MobId>, progress: &mut dyn FnMut(usize, usize, &str)) -> ScanResult {
    let all: Vec<(usize, PathBuf)> = projects.iter().enumerate().flat_map(|(i, p)| bins(p).into_iter().map(move |b| (i, b))).collect();
    let mut r = ScanResult { projects: projects.len(), bins: all.len(), ..Default::default() };
    let mut missing: HashMap<MobId, Missing> = HashMap::new();
    let mut by_bin: BTreeMap<(String, String), BTreeMap<MobId, BTreeSet<(String, ClipKind)>>> = BTreeMap::new();
    // missing file mobs: locator class, name, AMA, source; and the sources of the files that are on the drives
    let mut files: HashMap<MobId, (Option<String>, String, bool, Option<(String, bool)>)> = HashMap::new();
    let mut on_drives: HashSet<(String, bool)> = HashSet::new();
    let source = |bin: &Bin, m: &crate::avb::Mob| bin.source_name(m).map(|n| (n, matches!(m.descriptor.as_deref(), Some("PCMA" | "WAVE" | "AIFC"))));
    let mut renders: HashSet<MobId> = HashSet::new();
    // missing file -> the master clip it belongs to
    let mut owners: HashMap<MobId, String> = HashMap::new();
    // per project: master/subclip mob -> media files, and uses still waiting for a mob from another bin
    let mut media_of: HashMap<MobId, Vec<MobId>> = HashMap::new();
    let mut waiting: Vec<(MobId, Use)> = vec![];
    let mut current = usize::MAX;
    // (project, key of the name) of master clips whose media are all on the drives
    let mut online_masters: HashSet<(String, String)> = HashSet::new();

    let mut record = |id: MobId, u: Use, missing: &mut HashMap<MobId, Missing>| {
        if available.contains(&id) {
            return;
        }
        by_bin.entry((u.project.clone(), u.bin.clone())).or_default().entry(id).or_default().insert((u.clip.clone(), u.kind));
        if u.kind == ClipKind::Master {
            owners.entry(id).or_insert_with(|| u.clip.clone());
        }
        missing.entry(id).or_insert_with(|| Missing { id, name: String::new(), unmanaged: false, relink: false, first: u });
    };
    let finish_project = |media_of: &mut HashMap<MobId, Vec<MobId>>, waiting: &mut Vec<(MobId, Use)>, missing: &mut HashMap<MobId, Missing>, record: &mut dyn FnMut(MobId, Use, &mut HashMap<MobId, Missing>)| {
        for (mob, u) in waiting.drain(..) {
            for id in media_of.get(&mob).into_iter().flatten() {
                record(*id, u.clone(), missing);
            }
        }
        media_of.clear();
    };

    for (i, (pi, path)) in all.iter().enumerate() {
        if *pi != current {
            finish_project(&mut media_of, &mut waiting, &mut missing, &mut record);
            current = *pi;
        }
        let project = &projects[*pi];
        let rel = path.strip_prefix(project).map(|p| p.display().to_string()).unwrap_or_else(|_| name_of(path));
        progress(i, all.len(), &format!("{}/{rel}", name_of(project)));
        let bin = match Bin::read(path) {
            Ok(b) => b,
            Err(e) => {
                r.failed.push((path.clone(), e));
                continue;
            }
        };
        for m in bin.mobs.iter().filter(|m| m.is_file()) {
            if available.contains(&m.id) {
                on_drives.extend(source(&bin, m));
            } else {
                files.entry(m.id).or_insert_with(|| (m.locator.clone(), m.name.clone(), m.ama, source(&bin, m)));
            }
        }
        for &item in &bin.items {
            let mob = &bin.mobs[item];
            if mob.kind == MobKind::Master && mob.usage == 1 {
                let mut unresolved = HashSet::new();
                renders.extend(bin.media_of(item, &mut unresolved).into_iter().filter(|id| !available.contains(id)));
                continue;
            }
            let Some(kind) = kind_of(mob.kind, mob.usage) else { continue };
            r.clips += 1;
            let u = Use { project: name_of(project), bin: rel.clone(), clip: mob.name.clone(), kind };
            let mut unresolved = HashSet::new();
            let media = bin.media_of(item, &mut unresolved);
            if kind == ClipKind::Master && !media.is_empty() && unresolved.is_empty() && media.iter().all(|id| available.contains(id)) {
                online_masters.insert((u.project.clone(), crate::key(&mob.name)));
            }
            if kind != ClipKind::Sequence {
                media_of.insert(mob.id, media.clone());
            }
            for id in media {
                record(id, u.clone(), &mut missing);
            }
            waiting.extend(unresolved.into_iter().map(|id| (id, u.clone())));
        }
    }
    finish_project(&mut media_of, &mut waiting, &mut missing, &mut record);
    drop(record);

    // named after its master clip (file mobs are often called just "File"), else the file mob, else the first use.
    // shortcut: relink matches source name and picture/sound only, not the timecode range – a newer file that
    // covers less of the source still counts; compare timecodes if that shows up in practice.
    renders.retain(|id| !missing.contains_key(id));
    let is_ama = |id: &MobId| files.get(id).is_some_and(|f| f.2);
    let mut ama_names: HashMap<MobId, String> = missing
        .iter()
        .filter(|(id, _)| is_ama(id))
        .map(|(id, m)| (*id, owners.get(id).cloned().or_else(|| files.get(id).map(|f| f.1.clone()).filter(|n| !n.trim().is_empty())).unwrap_or_else(|| m.first.clip.clone())))
        .collect();
    let before = ama_names.len();
    ama_names.retain(|id, name| !online_masters.contains(&(missing[id].first.project.clone(), crate::key(name))));
    r.ama_covered = before - ama_names.len();
    r.ama = ama_names.len();
    missing.retain(|id, _| !is_ama(id));
    let no_locator = |id: &MobId| files.get(id).is_some_and(|f| f.0.is_none());
    r.no_media = missing.keys().filter(|id| no_locator(id)).count();
    missing.retain(|id, _| !no_locator(id));
    for m in missing.values_mut() {
        let (locator, name, _, src) = files.remove(&m.id).unwrap_or((Some("MSML".into()), String::new(), false, None));
        m.unmanaged = locator.as_deref() != Some("MSML");
        m.relink = src.is_some_and(|s| on_drives.contains(&s));
        m.name = owners.remove(&m.id).or(Some(name).filter(|n| !n.trim().is_empty())).unwrap_or_else(|| m.first.clip.clone());
    }
    r.renders = renders.len();
    // per bin: the files `info` knows (name, relink), grouped by name with all their uses
    let per_bin = |info: &dyn Fn(&MobId) -> Option<(String, bool)>| -> BTreeMap<(String, String), Vec<MissingGroup>> {
        by_bin
            .iter()
            .map(|(k, ids)| {
                let mut groups: BTreeMap<(String, bool), (usize, BTreeSet<(String, ClipKind)>)> = BTreeMap::new();
                for (id, uses) in ids {
                    let Some(key) = info(id) else { continue };
                    let g = groups.entry(key).or_default();
                    g.0 += 1;
                    g.1.extend(uses.iter().cloned());
                }
                let groups: Vec<MissingGroup> = groups.into_iter().map(|((name, relink), (files, uses))| MissingGroup { name, files, relink, uses: uses.into_iter().collect() }).collect();
                (k.clone(), groups)
            })
            .filter(|(_, g)| !g.is_empty())
            .collect()
    };
    r.by_bin = per_bin(&|id| missing.get(id).map(|m| (m.name.clone(), m.relink)));
    r.ama_by_bin = per_bin(&|id| ama_names.get(id).map(|n| (n.clone(), false)));
    r.relink = missing.values().filter(|m| m.relink).count();
    let mut list: Vec<Missing> = missing.into_values().filter(|m| !m.relink).collect();
    list.sort_by(|a, b| (&a.first.project, &a.first.bin, &a.first.clip, a.id).cmp(&(&b.first.project, &b.first.bin, &b.first.clip, b.id)));
    r.missing = list;
    r
}

/// Request for MediaFiles Ferry (format "mediafiles-ferry-request", see Ferry's src/request.rs).
#[derive(Serialize)]
struct Request<'a> {
    format: &'a str,
    version: u32,
    system: &'a str,
    created: u64,
    files: Vec<Wanted<'a>>,
}

#[derive(Serialize)]
struct Wanted<'a> {
    id: String,
    clip: &'a str,
    bin: &'a str,
    project: &'a str,
}

pub fn request_json(system: &str, created: u64, missing: &[Missing]) -> Vec<u8> {
    let files = missing
        .iter()
        .map(|m| Wanted { id: id_hex(&m.id), clip: &m.first.clip, bin: &m.first.bin, project: &m.first.project })
        .collect();
    serde_json::to_vec(&Request { format: "mediafiles-ferry-request", version: 1, system, created, files }).unwrap_or_default()
}

/// The missing media as text: per project and bin the clips and their missing files (ids).
pub fn list_text(r: &ScanResult, lang: &str) -> String {
    let t = |texts: [&str; 4]| crate::pick(lang, texts).to_string();
    let mut out = t([
        "MediaFiles Check – {n} media files missing ({p} projects, {b} bins, {c} clips checked)",
        "MediaFiles Check – {n} Mediendateien fehlen ({p} Projekte, {b} Bins, {c} Clips geprüft)",
        "MediaFiles Check – {n} fichiers médias manquants ({p} projets, {b} chutiers, {c} clips vérifiés)",
        "MediaFiles Check – faltan {n} archivos de medios ({p} proyectos, {b} bins, {c} clips comprobados)",
    ])
    .replace("{n}", &r.missing.len().to_string())
    .replace("{p}", &r.projects.to_string())
    .replace("{b}", &r.bins.to_string())
    .replace("{c}", &r.clips.to_string())
        + "\n";
    let kind = |k: &ClipKind| match k {
        ClipKind::Master => String::new(),
        ClipKind::Subclip => t([" (subclip)", " (Subclip)", " (sous-clip)", " (subclip)"]),
        ClipKind::Sequence => t([" (sequence)", " (Sequenz)", " (séquence)", " (secuencia)"]),
    };
    let bins = |out: &mut String, by_bin: &BTreeMap<(String, String), Vec<MissingGroup>>| {
        for ((project, bin), groups) in by_bin {
            out.push_str(&format!("\n{project} / {bin}\n"));
            for g in groups {
                let files = if g.files > 1 { format!(" ({})", t(["{} files", "{} Dateien", "{} fichiers", "{} archivos"]).replace("{}", &g.files.to_string())) } else { String::new() };
                let relink = if g.relink { t([" – on the drives, relink", " – auf den Laufwerken, neu verlinken", " – sur les disques, à relier", " – en los discos, revincular"]) } else { String::new() };
                out.push_str(&format!("  {}{files}{relink}\n", g.name));
                for (clip, k) in &g.uses {
                    out.push_str(&format!("      {clip}{}\n", kind(k)));
                }
            }
        }
    };
    bins(&mut out, &r.by_bin);
    if !r.ama_by_bin.is_empty() {
        out.push_str(&t(["\n\nAMA-linked clips per bin:\n", "\n\nAMA-verlinkte Clips pro Bin:\n", "\n\nClips liés par AMA par chutier :\n", "\n\nClips vinculados por AMA por bin:\n"]));
        bins(&mut out, &r.ama_by_bin);
    }
    if r.renders > 0 {
        out.push_str(&t([
            "\n{} render files are missing too – not requested, Media Composer renders them again.\n",
            "\nAußerdem fehlen {} Render-Dateien – nicht angefordert, Media Composer rendert sie neu.\n",
            "\n{} fichiers de rendu manquent aussi – non demandés, Media Composer les recalcule.\n",
            "\nTambién faltan {} archivos de render: no se solicitan, Media Composer los vuelve a renderizar.\n",
        ]).replace("{}", &r.renders.to_string()));
    }
    if r.relink > 0 {
        out.push_str(&t([
            "\n{} missing files are on the drives as another media file of the same source – relink them in Media Composer; not requested.\n",
            "\n{} fehlende Dateien liegen als andere Mediendatei derselben Quelle auf den Laufwerken – in Media Composer neu verlinken; nicht angefordert.\n",
            "\n{} fichiers manquants sont sur les disques sous un autre fichier média de la même source – reliez-les dans Media Composer ; non demandés.\n",
            "\n{} archivos que faltan están en los discos como otro archivo de medios de la misma fuente: revincúlalos en Media Composer; no se solicitan.\n",
        ]).replace("{}", &r.relink.to_string()));
    }
    if r.ama > 0 {
        out.push_str(&t([
            "\n{} AMA-linked files left out.\n",
            "\n{} AMA-verlinkte Dateien ausgelassen.\n",
            "\n{} fichiers liés par AMA ignorés.\n",
            "\n{} archivos vinculados por AMA omitidos.\n",
        ]).replace("{}", &r.ama.to_string()));
    }
    if r.ama_covered > 0 {
        out.push_str(&t([
            "\n{} more AMA-linked files have a master clip of the same name with its media on the drives – left out.\n",
            "\n{} weitere AMA-verlinkte Dateien haben einen gleichnamigen Masterclip mit Medien auf den Laufwerken – ausgelassen.\n",
            "\n{} autres fichiers liés par AMA ont un clip master du même nom avec ses médias sur les disques – ignorés.\n",
            "\n{} archivos vinculados por AMA más tienen un clip master con el mismo nombre y sus medios en los discos: omitidos.\n",
        ]).replace("{}", &r.ama_covered.to_string()));
    }
    if r.no_media > 0 {
        out.push_str(&t([
            "\n{} references without media left out (e.g. camera sound that was never imported).\n",
            "\n{} Verweise ohne Mediendatei ausgelassen (z. B. nie importierter Kameraton).\n",
            "\n{} références sans fichier média ignorées (p. ex. son caméra jamais importé).\n",
            "\n{} referencias sin archivo de medios omitidas (p. ej. sonido de cámara nunca importado).\n",
        ]).replace("{}", &r.no_media.to_string()));
    }
    let unmanaged = r.missing.iter().filter(|m| m.unmanaged).count();
    if unmanaged > 0 {
        out.push_str(&t([
            "\n{} of the missing files have no Avid media reference – e.g. imported via AAF.\n",
            "\n{} der fehlenden Dateien haben keinen Avid-Medienverweis – z. B. über AAF importiert.\n",
            "\n{} des fichiers manquants n'ont pas de référence média Avid – p. ex. importés par AAF.\n",
            "\n{} de los archivos que faltan no tienen referencia de medios de Avid: p. ej. importados por AAF.\n",
        ]).replace("{}", &unmanaged.to_string()));
    }
    for (p, e) in &r.failed {
        out.push_str(&format!("\n! {}: {e}", p.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_folders_with_an_avp_and_bins_skip_the_attic() {
        let root = std::env::temp_dir().join(format!("mfc-projects-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["Avid Projects/A/AvidSharedData/user", "Avid Projects/A/Bins/Attic", "Avid Projects/Archiv/B", "Avid Projects/C"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["Avid Projects/A/A.avp", "Avid Projects/A/AvidSharedData/user/A.avp", "Avid Projects/Archiv/B/B.avp", "Avid Projects/A/Reel 1.avb", "Avid Projects/A/Bins/Cuts.avb", "Avid Projects/A/Bins/Attic/Cuts.avb.1"] {
            fs::write(root.join(f), b"").unwrap();
        }
        let projects: Vec<String> = find_projects(&root.join("Avid Projects")).iter().map(|p| name_of(p)).collect();
        assert_eq!(projects, ["A", "B"]); // C has no .avp, AvidSharedData is inside A
        let b: Vec<String> = bins(&root.join("Avid Projects/A")).iter().map(|p| name_of(p)).collect();
        assert_eq!(b, ["Cuts.avb", "Reel 1.avb"]);
        // unreadable bins are reported, not fatal
        let r = scan(&[root.join("Avid Projects/A")], &HashSet::new(), &mut |_, _, _| {});
        assert_eq!((r.bins, r.failed.len(), r.missing.len()), (2, 2, 0));
        fs::remove_dir_all(&root).unwrap();
    }
}
