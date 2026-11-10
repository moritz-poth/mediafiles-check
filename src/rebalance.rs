//! Rebalancing: keeps MXF folders at most at the limit (Settings → Rebalancing, default 5000). The limit counts every
//! file of a folder, so a folder takes `limit - 2` media files: the two database files come on top.
//!
//! A folder and its "<base>-rebalanced-<n>" folders form a group (found by name, so it also works when another
//! workstation rebalanced; base = the name without a Media Composer ending ".<number>": "Mac Studio.1" →
//! "Mac Studio-rebalanced-1", "1" → "1-rebalanced-1"). Every run lays the whole group out anew from what is on the
//! drive: all its MXF files sorted by creation date, the oldest `limit - 2` into "-rebalanced-1", the next into "-2" …,
//! the youngest stay in the main folder. A lower limit spreads the files over more folders, a higher one moves them
//! back; rebalanced folders that end without media files are removed. Because the layout comes from the drive, a run
//! that stopped half-way is simply finished by the next one.
//! Media Composer's own folders ("1" local, "<computer>.<n>" on network drives) should get a name of their own first:
//! the main folder is renamed (one rename, nothing copied) and the group gets the new name.
//! Only MXF files move; within one volume a move is a rename, nothing is copied or deleted. Afterwards the databases
//! of every changed folder are rewritten (set aside + "Rename for Media Composer", rename.rs).
use crate::{MDB, PMR, key};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const DEFAULT_LIMIT: usize = 5000;
/// what Settings accepts
pub const LIMITS: std::ops::RangeInclusive<usize> = 100..=50_000;

/// Media files one folder takes: the limit minus the two database files.
pub fn per_folder(limit: usize) -> usize {
    limit.saturating_sub(2).max(1)
}

/// "Mac Studio.1" → "Mac Studio", "1" → "1": Media Composer's ".<number>" ending is dropped, a bare number stays.
pub fn base_name(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((base, n)) if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    }
}

/// "1-rebalanced-3" → ("1", 3)
pub fn member_of(name: &str) -> Option<(&str, usize)> {
    let (base, n) = name.rsplit_once("-rebalanced-")?;
    if base.is_empty() || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((base, n.parse().ok()?))
}

fn created(m: &fs::Metadata) -> u128 {
    // creation date (kept by the Finder when copying); the modification date where a file system has none
    m.created().or_else(|_| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// MXF files of a folder: (creation date, name); a missing folder has none.
fn mxf_dated(dir: &Path) -> io::Result<Vec<(u128, String)>> {
    let mut v = vec![];
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(v),
        Err(e) => return Err(e),
    };
    for e in rd {
        let e = e?;
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with('.') && n.to_lowercase().ends_with(".mxf") && e.file_type()?.is_file() {
            v.push((created(&e.metadata()?), n));
        }
    }
    Ok(v)
}

fn subdirs(mxf: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(mxf)
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The rebalanced folders of `main` in this MXF folder, by number.
pub fn members(mxf: &Path, main: &str) -> Vec<String> {
    let base = key(base_name(main));
    let mut v: Vec<(usize, String)> = subdirs(mxf).into_iter().filter_map(|d| member_of(&d).filter(|(b, _)| key(b) == base).map(|(_, n)| (n, d.clone()))).collect();
    v.sort();
    v.into_iter().map(|m| m.1).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    pub file: String,
    pub from: String,
    pub to: String,
}

/// How a group would be laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub mxf: PathBuf,
    /// the main folder's name now
    pub main: String,
    /// its rebalanced folders now
    pub members: Vec<String>,
    /// MXF files in the whole group
    pub count: usize,
    /// new name for the main folder (asked before a run of a Media Composer folder)
    pub rename_to: Option<String>,
    /// rebalanced folders afterwards (oldest first) with their number of media files; the main folder keeps `stays`
    pub folders: Vec<(String, usize)>,
    pub stays: usize,
    pub moves: Vec<Move>,
    /// rebalanced folders that end without media files (they are removed)
    pub emptied: Vec<String>,
    /// the main folder is one Media Composer creates itself: renaming it is recommended
    pub mc_folder: bool,
    /// a folder of the group is renamed for Media Composer right now: rename it back first
    pub blocked: bool,
}

impl Plan {
    pub fn final_main(&self) -> &str {
        self.rename_to.as_deref().unwrap_or(&self.main)
    }

    /// Folders whose content changes – their databases have to be rewritten (names after the run).
    pub fn changed(&self) -> Vec<String> {
        let main = self.final_main().to_string();
        let name = |f: &str| if f == self.main { main.clone() } else { f.to_string() };
        let mut v: Vec<String> = vec![];
        for m in &self.moves {
            for f in [name(&m.from), name(&m.to)] {
                if !v.contains(&f) && !self.emptied.contains(&f) {
                    v.push(f);
                }
            }
        }
        if self.rename_to.is_some() && !v.contains(&main) {
            v.push(main);
        }
        v
    }

    /// Something to do: files to move or a new name.
    pub fn needed(&self) -> bool {
        !self.moves.is_empty() || self.rename_to.is_some()
    }
}

/// Lays out the group of `main` for `per` media files per folder. `reserved` holds folder names (keys) already
/// planned or in use elsewhere in the same MXF folder (e.g. renamed for Media Composer), so nothing picks them twice.
pub fn plan(mxf: &Path, main: &str, per: usize, rename_to: Option<&str>, mc_folder: bool, reserved: &mut HashSet<String>) -> io::Result<Plan> {
    let members = members(mxf, main);
    let mut files: Vec<(u128, String, String)> = vec![];
    for folder in std::iter::once(main.to_string()).chain(members.iter().cloned()) {
        files.extend(mxf_dated(&mxf.join(&folder))?.into_iter().map(|(t, n)| (t, n, folder.clone())));
    }
    files.sort();
    let per = per.max(1);
    let k = if files.len() <= per { 0 } else { (files.len() - 1) / per };
    let base = rename_to.map_or_else(|| base_name(main).to_string(), str::to_string);
    let taken: HashSet<String> = subdirs(mxf).iter().map(|d| key(d)).collect();
    // the existing rebalanced folders keep their names, unless the group gets a new name
    let mut targets: Vec<String> = if rename_to.is_none() { members.iter().take(k).cloned().collect() } else { vec![] };
    let mut n = 0;
    while targets.len() < k {
        n += 1;
        let c = format!("{base}-rebalanced-{n}");
        if !taken.contains(&key(&c)) && !reserved.contains(&key(&c)) && !targets.contains(&c) {
            targets.push(c);
        }
    }
    targets.sort_by_key(|t| member_of(t).map_or(0, |m| m.1));
    for t in &targets {
        reserved.insert(key(t));
    }
    let mut moves = vec![];
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (i, (_, file, from)) in files.iter().enumerate() {
        let to = if i / per < k { targets[i / per].clone() } else { main.to_string() };
        *counts.entry(to.clone()).or_default() += 1;
        if &to != from {
            moves.push(Move { file: file.clone(), from: from.clone(), to });
        }
    }
    let emptied = members.iter().filter(|m| !targets.contains(m)).cloned().collect();
    Ok(Plan {
        mxf: mxf.to_path_buf(),
        main: main.to_string(),
        members,
        count: files.len(),
        rename_to: rename_to.map(str::to_string),
        folders: targets.iter().map(|t| (t.clone(), counts.get(t).copied().unwrap_or(0))).collect(),
        stays: counts.get(main).copied().unwrap_or(0),
        moves,
        emptied,
        mc_folder,
        blocked: false,
    })
}

/// Groups on these drives that do not match the limit (a folder over it, or rebalanced folders laid out for another
/// limit). `renamed` = (MXF folder, original name) of folders renamed for Media Composer right now: their group is
/// marked blocked and their names stay reserved. `is_mc` tells Media Composer's own folder names.
pub fn full_folders(volumes: &[PathBuf], limit: usize, renamed: &[(PathBuf, String)], is_mc: &dyn Fn(&Path, &str) -> bool) -> Vec<Plan> {
    let per = per_folder(limit);
    let mut plans = vec![];
    for v in volumes {
        let mxf = crate::mxf_dir(v);
        let dirs = subdirs(&mxf);
        let away: Vec<&String> = renamed.iter().filter(|r| r.0 == mxf).map(|r| &r.1).collect();
        let mut reserved: HashSet<String> = away.iter().map(|n| key(n)).collect();
        let bases: HashSet<String> = dirs.iter().chain(away.iter().copied()).filter(|d| member_of(d).is_none()).map(|d| key(base_name(d))).collect();
        // a rebalanced folder whose main folder exists belongs to that group; one without stands for itself
        let mains: Vec<&String> = dirs.iter().filter(|d| member_of(d).is_none_or(|(b, _)| !bases.contains(&key(b)))).collect();
        let mut seen: HashSet<String> = HashSet::new();
        for main in mains {
            // "Mac Studio" and "Mac Studio.1" would share their rebalanced folders: only the first takes them
            if !seen.insert(key(base_name(main))) {
                continue;
            }
            let Ok(mut p) = plan(&mxf, main, per, None, is_mc(&mxf, main), &mut reserved) else { continue };
            let base = key(base_name(main));
            p.blocked = away.iter().any(|a| key(base_name(a)) == base || member_of(a).is_some_and(|(b, _)| key(b) == base));
            if p.needed() {
                plans.push(p);
            }
        }
    }
    plans
}

/// Why a new name for a main folder is not possible.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum NameProblem {
    Empty,
    BadCharacters,
    Exists,
    MediaComposer,
    Reserved,
}

/// `is_mc`: the name is one Media Composer uses itself; `reserved` = names other plans already took.
pub fn check_name(mxf: &Path, name: &str, is_mc: bool, reserved: &HashSet<String>) -> Option<NameProblem> {
    let n = name.trim();
    if n.is_empty() {
        Some(NameProblem::Empty)
    } else if n.starts_with('.') || n.contains('/') || n.contains(':') || n != name {
        Some(NameProblem::BadCharacters)
    } else if member_of(n).is_some() {
        Some(NameProblem::Reserved)
    } else if is_mc {
        Some(NameProblem::MediaComposer)
    } else if mxf.join(n).exists() || subdirs(mxf).iter().any(|d| key(d) == key(n)) || reserved.contains(&key(n)) {
        Some(NameProblem::Exists)
    } else {
        None
    }
}

/// One group in the history (one entry per group; a new run of the same group updates it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rebalanced {
    /// the MXF folder that holds the group
    pub mxf: PathBuf,
    /// the main folder's name (after renaming)
    pub original: String,
    /// first beta 7 builds: new folder → files moved there; only read from old entries
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<(String, Vec<(String, u64)>)>,
    /// local time of the last run, for the history
    pub date: String,
    /// "Start" (databases set aside, folders renamed for Media Composer) was done; until then the history offers it.
    /// Journals from before this field count as done (new entries always write it).
    #[serde(default = "yes")]
    pub db_done: bool,
    /// the main folder's name before rebalancing renamed it (undo gives it back)
    #[serde(default)]
    pub renamed_from: Option<String>,
    /// rebalanced folders after the last run
    #[serde(default)]
    pub folders: Vec<String>,
    /// media files moved in the last run
    #[serde(default)]
    pub files: usize,
    /// folders whose databases have to be rewritten
    #[serde(default)]
    pub changed: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Rebalanced {
    /// Folders whose databases "Start" rewrites (old entries: the main folder and the new folders).
    pub fn to_rewrite(&self) -> Vec<String> {
        if self.changed.is_empty() && !self.moved.is_empty() {
            return std::iter::once(self.original.clone()).chain(self.moved.iter().map(|m| m.0.clone())).collect();
        }
        self.changed.clone()
    }

    pub fn rebalanced_folders(&self) -> Vec<String> {
        if self.folders.is_empty() { self.moved.iter().map(|m| m.0.clone()).collect() } else { self.folders.clone() }
    }

    pub fn moved_files(&self) -> usize {
        if self.files == 0 { self.moved.iter().map(|m| m.1.len()).sum() } else { self.files }
    }

    /// After the drive came back under another path (e.g. "/Volumes/Media 1" after a crash): the MXF folder on another
    /// drive that holds the main folder and all its rebalanced folders. Updates `mxf`; true if it changed.
    pub fn relocate(&mut self) -> bool {
        let folders = self.rebalanced_folders();
        if folders.is_empty() || self.mxf.is_dir() {
            return false;
        }
        let names: Vec<&str> = std::iter::once(self.original.as_str()).chain(folders.iter().map(String::as_str)).collect();
        let found = crate::find_volumes().into_iter().map(|v| crate::mxf_dir(&v)).find(|m| names.iter().all(|n| m.join(n).is_dir()));
        found.map(|m| self.mxf = m).is_some()
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Journal {
    pub entries: Vec<Rebalanced>,
}

impl Journal {
    /// A missing journal is empty; an unreadable one is an error – it must never be overwritten blindly.
    pub fn load(path: &Path) -> io::Result<Journal> {
        match fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes next to the file first, then renames: a crash never leaves half a journal.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let tmp = path.with_extension("tmp");
        let mut f = File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self)?)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    }

    /// The entry of this group (by its main folder's name now).
    pub fn find(&self, mxf: &Path, main: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.mxf == mxf && key(&e.original) == key(main))
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Moved {
    pub moved: usize,
    /// files left where they are: a file of that name is in the target already, or it is gone
    pub skipped: Vec<String>,
    /// emptied rebalanced folders that still hold other files (kept)
    pub kept: Vec<String>,
}

/// Renames the main folder (if planned), moves the files, sets the databases of emptied rebalanced folders aside and
/// removes those folders when nothing else is in them. Never overwrites, never deletes a file.
fn apply(p: &Plan) -> io::Result<Moved> {
    let main = p.final_main().to_string();
    if let Some(to) = &p.rename_to {
        let target = p.mxf.join(to);
        if target.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists already", target.display())));
        }
        crate::move_new(&p.mxf.join(&p.main), &target)?;
    }
    let name = |f: &str| if f == p.main { main.clone() } else { f.to_string() };
    let mut out = Moved::default();
    for (folder, _) in &p.folders {
        fs::create_dir_all(p.mxf.join(folder))?;
    }
    for m in &p.moves {
        let (src, dst) = (p.mxf.join(name(&m.from)).join(&m.file), p.mxf.join(name(&m.to)).join(&m.file));
        if dst.exists() || !src.is_file() {
            out.skipped.push(m.file.clone());
            continue;
        }
        match crate::move_new(&src, &dst) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                out.skipped.push(m.file.clone());
                continue;
            }
            r => r?,
        }
        out.moved += 1;
    }
    for e in &p.emptied {
        let dir = p.mxf.join(e);
        if !mxf_dated(&dir)?.is_empty() {
            out.kept.push(e.clone());
            continue;
        }
        if is_leftover(&dir) {
            // only the database (and Media Composer's "Creating" folder) left: the whole folder goes aside with it
            move_leftover(&dir, &backup_dir(&dir)?)?;
        } else {
            set_aside_database(&dir)?;
            if dir.exists() {
                out.kept.push(e.clone());
            }
        }
    }
    Ok(out)
}

/// Runs a plan. The history entry of the group is saved before anything changes on the drive.
pub fn execute(p: &Plan, journal: &mut Journal, journal_path: &Path) -> io::Result<(Rebalanced, Moved)> {
    let main = p.final_main().to_string();
    let i = journal.find(&p.mxf, &p.main);
    let old = i.map(|i| journal.entries[i].clone());
    let mut changed = p.changed();
    if let Some(o) = old.as_ref().filter(|o| !o.db_done) {
        // the databases of the run before were not rewritten yet: they still have to be
        for c in o.to_rewrite() {
            let c = if c == p.main { main.clone() } else { c };
            if !changed.contains(&c) && !p.emptied.contains(&c) {
                changed.push(c);
            }
        }
    }
    let entry = Rebalanced {
        mxf: p.mxf.clone(),
        original: main.clone(),
        moved: vec![],
        date: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        db_done: false,
        renamed_from: old.as_ref().and_then(|o| o.renamed_from.clone()).or_else(|| p.rename_to.as_ref().map(|_| p.main.clone())),
        folders: p.folders.iter().map(|f| f.0.clone()).collect(),
        files: p.moves.len(),
        changed,
    };
    match i {
        Some(i) => journal.entries[i] = entry.clone(),
        None => journal.entries.push(entry.clone()),
    }
    journal.save(journal_path)?;
    let moved = apply(p)?;
    Ok((entry, moved))
}

/// Puts msmFMID.pmr and msmMMOB.mdb of a folder aside so Media Composer writes new ones: into
/// "<drive>/MediaFiles Check Database Backups/<time> <folder>/" – on the same drive (a rename, nothing is copied),
/// outside "Avid MediaFiles" (Media Composer never looks there). Returns the backup folder, None if there was nothing.
pub fn set_aside_database(folder: &Path) -> io::Result<Option<PathBuf>> {
    let present: Vec<&str> = [PMR, MDB].into_iter().filter(|f| folder.join(f).is_file()).collect();
    if present.is_empty() {
        return Ok(None);
    }
    let backup = backup_dir(folder)?;
    fs::create_dir_all(&backup)?;
    for f in present {
        if backup.join(f).exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists already", backup.join(f).display())));
        }
        crate::move_new(&folder.join(f), &backup.join(f))?;
    }
    Ok(Some(backup))
}

/// "<drive>/MediaFiles Check Database Backups/<time> <folder>" for a folder <drive>/Avid MediaFiles/MXF/<folder>.
fn backup_dir(folder: &Path) -> io::Result<PathBuf> {
    let drive = folder.parent().and_then(Path::parent).and_then(Path::parent).ok_or_else(|| io::Error::other("not in Avid MediaFiles/MXF"))?;
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    Ok(drive.join("MediaFiles Check Database Backups").join(format!("{} {name}", chrono::Local::now().format("%Y-%m-%d %H%M%S"))))
}

/// What may be left in a media folder without media files: the two database files, Media Composer's "Creating"
/// folder (without anything visible in it) and hidden files like .DS_Store.
pub fn is_leftover_entry(path: &Path) -> bool {
    let n = path.file_name().unwrap_or_default().to_string_lossy();
    n.starts_with('.')
        || n.eq_ignore_ascii_case(PMR)
        || n.eq_ignore_ascii_case(MDB)
        || (n.eq_ignore_ascii_case("Creating") && path.is_dir() && fs::read_dir(path).is_ok_and(|rd| rd.flatten().all(|e| e.file_name().to_string_lossy().starts_with('.'))))
}

/// A folder that holds nothing but leftovers (see is_leftover_entry), or nothing at all.
pub fn is_leftover(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|rd| rd.flatten().all(|e| is_leftover_entry(&e.path())))
}

/// Moves everything in a folder into `into` (created if needed) and removes the then empty folder. Checks every
/// name first: nothing is moved if one exists in `into` already.
pub fn move_leftover(dir: &Path, into: &Path) -> io::Result<()> {
    let names: Vec<_> = fs::read_dir(dir)?.flatten().map(|e| e.file_name()).collect();
    if let Some(n) = names.iter().find(|n| into.join(n).exists()) {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists already", into.join(n).display())));
    }
    fs::create_dir_all(into)?;
    for n in &names {
        crate::move_new(&dir.join(n), &into.join(n))?;
    }
    fs::remove_dir(dir)
}

#[derive(Debug, Default, PartialEq)]
pub struct Undone {
    pub moved_back: usize,
    pub skipped: Vec<String>,
    /// rebalanced folders that still hold other files and were kept
    pub kept: Vec<String>,
    /// the main folder got its old name back
    pub renamed_back: Option<String>,
}

/// Moves every media file of the group back into the main folder, removes the emptied rebalanced folders (their
/// databases set aside) and gives the main folder its old name back if rebalancing renamed it and the name is free.
/// The main folder's database is set aside too, so Media Composer rebuilds it with all files.
pub fn undo(r: &Rebalanced) -> io::Result<Undone> {
    let main = r.mxf.join(&r.original);
    if !main.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} not found – if it is renamed for Media Composer, rename it back first", main.display())));
    }
    if let Some(name) = r.rebalanced_folders().iter().find(|n| !r.mxf.join(n).is_dir()) {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{name} not found – if it is renamed for Media Composer, rename it back first")));
    }
    let p = plan(&r.mxf, &r.original, usize::MAX, None, false, &mut HashSet::new())?;
    let m = apply(&p)?;
    let mut out = Undone { moved_back: m.moved, skipped: m.skipped, kept: m.kept, renamed_back: None };
    set_aside_database(&main)?;
    if let Some(old) = &r.renamed_from
        && !r.mxf.join(old).exists()
    {
        crate::move_new(&main, &r.mxf.join(old))?;
        out.renamed_back = Some(old.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str, folder: &str, n: usize) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mfc-rebalance-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("Avid MediaFiles/MXF").join(folder);
        fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            // written one after another: creation dates in this order
            fs::write(dir.join(format!("f{i:02}.mxf")), vec![b'x'; i + 1]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        fs::write(dir.join(PMR), b"pmr").unwrap();
        fs::write(dir.join(MDB), b"mdb").unwrap();
        dir
    }

    fn count(dir: &Path) -> usize {
        mxf_dated(dir).unwrap().len()
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = mxf_dated(dir).unwrap().into_iter().map(|f| f.1).collect();
        v.sort();
        v
    }

    #[test]
    fn names() {
        assert_eq!(base_name("Mac Studio.1"), "Mac Studio");
        assert_eq!(base_name("1"), "1");
        assert_eq!(base_name("clips.v2"), "clips.v2");
        assert_eq!(base_name(".1"), ".1");
        assert_eq!(member_of("1-rebalanced-12"), Some(("1", 12)));
        assert_eq!(member_of("a-rebalanced-"), None);
        assert_eq!(member_of("-rebalanced-1"), None);
        assert_eq!(per_folder(5000), 4998);
    }

    #[test]
    fn lower_and_higher_limit_relay_the_group() {
        let dir = setup("relay", "1", 55);
        let mxf = dir.parent().unwrap().to_path_buf();
        let jp = mxf.join("../../rebalanced.json");
        let mut j = Journal::default();
        // limit 52: the oldest 50 into 1-rebalanced-1, 5 stay
        let p = plan(&mxf, "1", per_folder(52), None, true, &mut HashSet::new()).unwrap();
        assert_eq!((p.folders.clone(), p.stays), (vec![("1-rebalanced-1".to_string(), 50)], 5));
        execute(&p, &mut j, &jp).unwrap();
        assert_eq!(files(&mxf.join("1-rebalanced-1"))[0], "f00.mxf");
        assert_eq!(files(&dir), ["f50.mxf", "f51.mxf", "f52.mxf", "f53.mxf", "f54.mxf"]);
        // same limit again: nothing to do
        assert!(!plan(&mxf, "1", 50, None, true, &mut HashSet::new()).unwrap().needed());
        // lower limit: the 5 youngest of rebalanced-1 go back, nothing else moves
        let p = plan(&mxf, "1", 45, None, true, &mut HashSet::new()).unwrap();
        assert_eq!(p.moves.len(), 5);
        assert!(p.moves.iter().all(|m| m.from == "1-rebalanced-1" && m.to == "1"));
        execute(&p, &mut j, &jp).unwrap();
        assert_eq!((count(&mxf.join("1-rebalanced-1")), count(&dir)), (45, 10));
        // much lower: 10 per folder, oldest first
        let p = plan(&mxf, "1", 10, None, true, &mut HashSet::new()).unwrap();
        assert_eq!(p.folders.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["1-rebalanced-1", "1-rebalanced-2", "1-rebalanced-3", "1-rebalanced-4", "1-rebalanced-5"]);
        execute(&p, &mut j, &jp).unwrap();
        assert_eq!(files(&mxf.join("1-rebalanced-2"))[0], "f10.mxf");
        assert_eq!((count(&mxf.join("1-rebalanced-5")), count(&dir)), (10, 5));
        assert_eq!(j.entries.len(), 1, "one history entry per group");
        // higher limit: the files move back together, emptied folders go (their databases set aside)
        fs::write(mxf.join("1-rebalanced-5").join(PMR), b"db").unwrap();
        fs::create_dir(mxf.join("1-rebalanced-5/Creating")).unwrap();
        fs::write(mxf.join("1-rebalanced-5/.DS_Store"), b"").unwrap();
        fs::write(mxf.join("1-rebalanced-4/notes.txt"), b"mine").unwrap(); // not a leftover: kept
        let p = plan(&mxf, "1", 30, None, true, &mut HashSet::new()).unwrap();
        assert_eq!(p.emptied, ["1-rebalanced-2", "1-rebalanced-3", "1-rebalanced-4", "1-rebalanced-5"]);
        let (e, m) = execute(&p, &mut j, &jp).unwrap();
        assert!(m.kept == ["1-rebalanced-4"] && m.skipped.is_empty());
        assert!(!mxf.join("1-rebalanced-5").exists() && !mxf.join("1-rebalanced-2").exists());
        let backups: Vec<PathBuf> = fs::read_dir(mxf.join("../../MediaFiles Check Database Backups")).unwrap().flatten().map(|e| e.path()).collect();
        assert!(backups.iter().any(|b| b.join(PMR).exists() && b.join("Creating").is_dir()), "folder went aside with its database");
        fs::remove_dir_all(mxf.join("1-rebalanced-4")).unwrap();
        assert_eq!((count(&mxf.join("1-rebalanced-1")), count(&dir)), (30, 25));
        assert!(e.changed.contains(&"1-rebalanced-1".to_string()) && e.changed.contains(&"1".to_string()));
        assert!(!e.db_done);
        // undo: all back into "1"
        let u = undo(&j.entries[0]).unwrap();
        assert_eq!(u.moved_back, 30);
        assert_eq!(count(&dir), 55);
        assert!(!mxf.join("1-rebalanced-1").exists());
        fs::remove_dir_all(mxf.join("../..")).unwrap();
    }

    #[test]
    fn renamed_group_and_undo() {
        let dir = setup("rename", "1", 12);
        let mxf = dir.parent().unwrap().to_path_buf();
        let jp = mxf.join("../../rebalanced.json");
        let mut j = Journal::default();
        let p = plan(&mxf, "1", 5, None, true, &mut HashSet::new()).unwrap();
        execute(&p, &mut j, &jp).unwrap();
        fs::create_dir(mxf.join("Taken")).unwrap();
        let none = HashSet::new();
        assert_eq!(check_name(&mxf, "Taken", false, &none), Some(NameProblem::Exists));
        assert_eq!(check_name(&mxf, "x-rebalanced-1", false, &none), Some(NameProblem::Reserved));
        assert_eq!(check_name(&mxf, "2", true, &none), Some(NameProblem::MediaComposer));
        assert_eq!(check_name(&mxf, "a/b", false, &none), Some(NameProblem::BadCharacters));
        assert_eq!(check_name(&mxf, "", false, &none), Some(NameProblem::Empty));
        assert_eq!(check_name(&mxf, "Projekt", false, &none), None);
        // renamed: the main folder gets the name, the old rebalanced folders are folded into the new group
        let p = plan(&mxf, "1", 5, Some("Projekt"), true, &mut HashSet::new()).unwrap();
        assert_eq!(p.folders.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["Projekt-rebalanced-1", "Projekt-rebalanced-2"]);
        assert_eq!(p.emptied, ["1-rebalanced-1", "1-rebalanced-2"]);
        let (e, _) = execute(&p, &mut j, &jp).unwrap();
        assert!(!dir.exists() && !mxf.join("1-rebalanced-1").exists());
        assert_eq!((count(&mxf.join("Projekt")), count(&mxf.join("Projekt-rebalanced-1"))), (2, 5));
        assert_eq!((e.original.as_str(), e.renamed_from.as_deref()), ("Projekt", Some("1")));
        assert_eq!(j.entries.len(), 1);
        // a folder renamed for Media Composer blocks its group
        let drive = mxf.parent().unwrap().parent().unwrap().to_path_buf();
        let found = full_folders(&[drive], 4, &[(mxf.clone(), "Projekt-rebalanced-1".into())], &|_, n| n == "1");
        assert!(found.iter().any(|p| p.main == "Projekt" && p.blocked));
        // undo: all back, the old name returns
        let u = undo(&j.entries[0]).unwrap();
        assert_eq!((u.moved_back, u.renamed_back.as_deref()), (10, Some("1")));
        assert_eq!(count(&dir), 12);
        fs::remove_dir_all(mxf.join("../..")).unwrap();
    }

    #[test]
    fn finds_groups_and_never_overwrites() {
        let dir = setup("find", "Mac Studio.1", 9);
        let mxf = dir.parent().unwrap().to_path_buf();
        let drive = mxf.parent().unwrap().parent().unwrap().to_path_buf();
        fs::create_dir(mxf.join("Mac Studio-rebalanced-1")).unwrap(); // empty, from an earlier run
        let found = full_folders(std::slice::from_ref(&drive), 9, &[], &|_, _| false);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].main.as_str(), found[0].folders.clone(), found[0].stays), ("Mac Studio.1", vec![("Mac Studio-rebalanced-1".to_string(), 7)], 2));
        assert!(full_folders(std::slice::from_ref(&drive), 12, &[], &|_, _| false).iter().all(|p| p.moves.is_empty()));
        // same file name in the target already: left where it is
        fs::write(mxf.join("Mac Studio-rebalanced-1/f00.mxf"), b"other").unwrap();
        let p = plan(&mxf, "Mac Studio.1", 4, None, false, &mut HashSet::new()).unwrap();
        let mut j = Journal::default();
        let (_, m) = execute(&p, &mut j, &mxf.join("../../j.json")).unwrap();
        assert!(m.skipped.contains(&"f00.mxf".to_string()));
        assert!(dir.join("f00.mxf").exists() && fs::read(mxf.join("Mac Studio-rebalanced-1/f00.mxf")).unwrap() == b"other");
        // an old journal entry still reads
        let old = r#"{"entries":[{"mxf":"/V","original":"1","moved":[["1-rebalanced-1",[["a.mxf",1]]]],"date":""}]}"#;
        let e = &serde_json::from_str::<Journal>(old).unwrap().entries[0];
        assert!(e.db_done);
        assert_eq!((e.to_rewrite(), e.moved_files()), (vec!["1".to_string(), "1-rebalanced-1".into()], 1));
        fs::remove_dir_all(mxf.join("../..")).unwrap();
    }
}
