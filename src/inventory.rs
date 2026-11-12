//! Media inventory: every MXF file on the chosen drives with what its header (mxf::Info), its folder's database
//! (msmFMID.pmr) and the chosen projects (projects::references) say about it.
//!
//! Kind: a render (precompute) or media. The header decides where it knows (a render's source is Media Composer's
//! "Precompute Source Mob"; a file from another program is never a render), otherwise the projects (a render reached through an effect, media
//! through a clip). Nothing else is guessed: a file neither knows stays Unknown and is never moved as media or render.
//! Headers are read once and cached by path, size and modification date.
use crate::avb::MobId;
use crate::projects::Refs;
use crate::{PMR, key, mxf, read_pmr_entries};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Media,
    Render,
    Unknown,
}

/// Where the kind comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum From {
    Header,
    Project,
    Nothing,
}

/// Saved with a scan (History): the header (`info`) comes from the header cache again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub path: PathBuf,
    /// drive name and MXF folder name
    pub drive: String,
    pub folder: String,
    pub size: u64,
    /// modification date (ns)
    pub modified: u128,
    /// what the header says; None if it could not be read (`error`)
    #[serde(skip)]
    pub info: Option<mxf::Info>,
    pub error: Option<String>,
    /// file and master clip ID: from the header, else from the database
    #[serde(with = "hex_id")]
    pub file_id: Option<MobId>,
    #[serde(skip)]
    pub master_id: Option<MobId>,
    /// project named in the folder's database
    pub project: Option<String>,
    /// the folder's database lists the file
    pub in_db: bool,
    pub kind: Kind,
    pub kind_from: From,
    /// used by a clip, sequence or effect of the chosen projects; None when no project was chosen
    pub used: Option<bool>,
}

impl Item {
    pub fn name(&self) -> String {
        self.path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }
}

/// Raised when what `mxf::info` reads or decides changes: an older cache is read anew.
const CACHE_VERSION: u32 = 2;

/// A mob ID as 64 hex digits (a JSON array of 32 numbers would be three times as long).
mod hex_id {
    use crate::avb::MobId;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(id: &Option<MobId>, s: S) -> Result<S::Ok, S::Error> {
        match id {
            Some(id) => s.serialize_str(&crate::avb::id_hex(id)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<MobId>, D::Error> {
        let Some(t) = Option::<String>::deserialize(d)? else { return Ok(None) };
        let bytes: Vec<u8> = (0..t.len()).step_by(2).filter_map(|i| t.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
        Ok(bytes.try_into().ok())
    }
}

/// One scan kept for the History (`items` in a file of its own next to it).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ScanHead {
    pub date: String,
    pub drives: Vec<PathBuf>,
    pub projects: Vec<PathBuf>,
    /// projects whose bins were read, unreadable bins, media and renders used but on no scanned drive
    pub projects_read: usize,
    pub failed: usize,
    pub missing: (usize, usize),
    pub files: usize,
}

pub const SCANS_KEPT: usize = 5;

/// Saves a scan into `dir` (head + items) and removes all but the newest `SCANS_KEPT`.
pub fn save_scan(dir: &Path, head: &ScanHead, items: &[Item]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let base = chrono::Local::now().format("%Y%m%d-%H%M%S%3f").to_string();
    // two scans within the same millisecond (tests) must not share a file
    let stamp = (0..).map(|n| format!("{base}-{n:03}")).find(|s| !dir.join(format!("{s}.json")).exists()).unwrap();
    fs::write(dir.join(format!("{stamp}.items.json")), serde_json::to_vec(items)?)?;
    fs::write(dir.join(format!("{stamp}.json")), serde_json::to_vec_pretty(head)?)?;
    for (old, _) in list_scans(dir).into_iter().skip(SCANS_KEPT) {
        let _ = fs::remove_file(old.with_extension("items.json"));
        let _ = fs::remove_file(&old);
    }
    Ok(())
}

/// Saved scans, newest first: (head file, head).
pub fn list_scans(dir: &Path) -> Vec<(PathBuf, ScanHead)> {
    let mut v: Vec<(PathBuf, ScanHead)> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.to_string_lossy().ends_with(".items.json"))
                .filter_map(|p| Some((p.clone(), serde_json::from_slice(&fs::read(&p).ok()?).ok()?)))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// The items of a saved scan, their headers taken from the header cache (a header not there stays empty).
pub fn load_scan(head: &Path, cache: Option<&Path>) -> std::io::Result<Vec<Item>> {
    let mut items: Vec<Item> = serde_json::from_slice(&fs::read(head.with_extension("items.json"))?)?;
    let cached: Cache = cache.and_then(|c| fs::read(c).ok()).and_then(|b| serde_json::from_slice::<Cache>(&b).ok()).filter(|c| c.version == CACHE_VERSION).unwrap_or_default();
    for it in &mut items {
        if let Some((s, m, info)) = cached.files.get(&it.path.to_string_lossy().into_owned())
            && *s == it.size
            && *m == it.modified
        {
            it.master_id = info.master_id;
            it.info = Some(info.clone());
        }
    }
    Ok(items)
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    #[serde(default)]
    version: u32,
    /// path -> (size, modification date, header)
    files: HashMap<String, (u64, u128, mxf::Info)>,
}

fn modified(m: &fs::Metadata) -> u128 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// Lists and reads every MXF file in the MXF folders of `drives` (see crate::folders). `cache` (a JSON file) keeps
/// the headers for the next scan. progress gets (files done, files total); `stop` ends the scan early (the items
/// read so far are returned). Kind and use are set by `classify`.
pub fn scan(drives: &[PathBuf], cache: Option<&Path>, progress: &(dyn Fn(usize, usize) + Sync), stop: &AtomicBool) -> Vec<Item> {
    let mut items = vec![];
    for drive in drives {
        let drive_name = drive.file_name().map_or_else(|| drive.display().to_string(), |n| n.to_string_lossy().into_owned());
        for (_, dir) in crate::folders(drive) {
            let folder = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let db: HashMap<String, (MobId, MobId, String)> = fs::read(dir.join(PMR))
                .ok()
                .and_then(|d| read_pmr_entries(&d).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|e| (key(&e.name), (e.id, e.id2, e.project)))
                .collect();
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !name.to_lowercase().ends_with(".mxf") {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                if !m.is_file() {
                    continue;
                }
                let known = db.get(&key(&name));
                items.push(Item {
                    path: e.path(),
                    drive: drive_name.clone(),
                    folder: folder.clone(),
                    size: m.len(),
                    modified: modified(&m),
                    info: None,
                    error: None,
                    file_id: known.map(|k| k.0),
                    master_id: known.map(|k| k.1).filter(|id| *id != [0; 32]),
                    project: known.map(|k| k.2.clone()).filter(|p| !p.trim().is_empty()),
                    in_db: known.is_some(),
                    kind: Kind::Unknown,
                    kind_from: From::Nothing,
                    used: None,
                });
            }
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));

    let mut cached: Cache = cache.and_then(|c| fs::read(c).ok()).and_then(|b| serde_json::from_slice::<Cache>(&b).ok()).filter(|c| c.version == CACHE_VERSION).unwrap_or_default();
    cached.version = CACHE_VERSION;
    let todo: Vec<usize> = (0..items.len())
        .filter(|&i| {
            let it = &items[i];
            match cached.files.get(&it.path.to_string_lossy().into_owned()) {
                Some((s, m, info)) if *s == it.size && *m == it.modified => {
                    items[i].info = Some(info.clone());
                    false
                }
                _ => true,
            }
        })
        .collect();
    let (total, done) = (items.len(), AtomicUsize::new(items.len() - todo.len()));
    progress(done.load(Ordering::Relaxed), total);
    // headers are small; a few readers in parallel keep a network drive busy. In chunks: after each one the cache is
    // saved (at most once a minute), so a long first scan that is stopped keeps what it read.
    let mut saved = std::time::Instant::now();
    for chunk in todo.chunks(500) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<(usize, Result<mxf::Info, String>)>> = Mutex::new(vec![]);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = chunk.get(n) else { break };
                        let r = mxf::file_info(&items[i].path);
                        results.lock().unwrap().push((i, r));
                        progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                    }
                });
            }
        });
        for (i, r) in results.into_inner().unwrap() {
            match r {
                Ok(info) => {
                    cached.files.insert(items[i].path.to_string_lossy().into_owned(), (items[i].size, items[i].modified, info.clone()));
                    items[i].info = Some(info);
                }
                Err(e) => items[i].error = Some(e),
            }
        }
        if let Some(c) = cache.filter(|_| saved.elapsed() > std::time::Duration::from_secs(60)) {
            save_cache(c, &cached);
            saved = std::time::Instant::now();
        }
    }
    if stop.load(Ordering::Relaxed) {
        items.retain(|it| it.info.is_some() || it.error.is_some());
    }
    for it in &mut items {
        if let Some(info) = &it.info {
            it.file_id = info.file_id.or(it.file_id);
            it.master_id = info.master_id.or(it.master_id);
        }
    }
    if let Some(c) = cache {
        // forget files that are gone from the scanned drives
        let scanned: Vec<String> = drives.iter().map(|d| d.to_string_lossy().into_owned()).collect();
        let present: std::collections::HashSet<String> = items.iter().map(|i| i.path.to_string_lossy().into_owned()).collect();
        cached.files.retain(|p, _| present.contains(p) || !scanned.iter().any(|d| p.starts_with(d.as_str())));
        save_cache(c, &cached);
    }
    items
}

/// Comfort only: without the cache the next scan reads the headers again.
fn save_cache(c: &Path, cached: &Cache) {
    if let Ok(b) = serde_json::to_vec(cached) {
        let tmp = c.with_extension("tmp");
        if fs::write(&tmp, b).is_ok() {
            let _ = fs::rename(&tmp, c);
        }
    }
}

/// Sets kind and use of every item from its header and, if projects were read, from `refs`.
pub fn classify(items: &mut [Item], refs: Option<&Refs>) {
    for it in items {
        let in_refs = |set: fn(&Refs) -> &std::collections::HashSet<MobId>| refs.zip(it.file_id).is_some_and(|(r, id)| set(r).contains(&id));
        let (render_ref, media_ref) = (in_refs(|r| &r.renders), in_refs(|r| &r.media));
        (it.kind, it.kind_from) = match it.info.as_ref().and_then(|i| i.render) {
            Some(true) => (Kind::Render, From::Header),
            Some(false) => (Kind::Media, From::Header),
            None if render_ref => (Kind::Render, From::Project),
            None if media_ref => (Kind::Media, From::Project),
            None => (Kind::Unknown, From::Nothing),
        };
        it.used = refs.map(|_| render_ref || media_ref);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(render: Option<bool>, id: u8) -> Item {
        Item {
            path: PathBuf::from(format!("/V/{id}.mxf")),
            drive: "V".into(),
            folder: "1".into(),
            size: 1,
            modified: 0,
            info: Some(mxf::Info { render, ..Default::default() }),
            error: None,
            file_id: Some([id; 32]),
            master_id: None,
            project: None,
            in_db: true,
            kind: Kind::Unknown,
            kind_from: From::Nothing,
            used: None,
        }
    }

    #[test]
    fn scans_are_kept_and_loaded() {
        let dir = std::env::temp_dir().join(format!("mfc-scans-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let head = |n| ScanHead { date: format!("d{n}"), drives: vec![], projects: vec![], projects_read: 0, failed: 0, missing: (0, 0), files: n };
        for n in 0..7 {
            save_scan(&dir, &head(n), &[item(Some(true), n as u8)]).unwrap();
        }
        let list = list_scans(&dir);
        assert_eq!(list.iter().map(|l| l.1.files).collect::<Vec<_>>(), [6, 5, 4, 3, 2]);
        let items = load_scan(&list[0].0, None).unwrap();
        assert_eq!((items[0].file_id, items[0].kind, items[0].info.is_none()), (Some([6; 32]), Kind::Unknown, true));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 10);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn header_first_then_projects_never_guessed() {
        let mut items = vec![item(Some(true), 1), item(Some(false), 2), item(None, 3), item(None, 4), item(None, 5)];
        classify(&mut items, None);
        assert_eq!(items.iter().map(|i| (i.kind, i.used)).collect::<Vec<_>>()[..3], [(Kind::Render, None), (Kind::Media, None), (Kind::Unknown, None)]);
        let refs = Refs { media: [[2; 32], [4; 32]].into(), renders: [[3; 32]].into(), ..Default::default() };
        classify(&mut items, Some(&refs));
        let got: Vec<_> = items.iter().map(|i| (i.kind, i.kind_from, i.used)).collect();
        assert_eq!(
            got,
            [
                (Kind::Render, From::Header, Some(false)),
                (Kind::Media, From::Header, Some(true)),
                (Kind::Render, From::Project, Some(true)),
                (Kind::Media, From::Project, Some(true)),
                (Kind::Unknown, From::Nothing, Some(false)),
            ]
        );
    }
}
