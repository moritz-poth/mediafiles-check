//! Duplicates: media files that are more than once on the selected drives.
//!
//! A media file is identified by its file mob ID (the Media Composer ID in the MXF header, `mxf::file_mob_id`; if the
//! header cannot be read, the ID its folder's msmFMID.pmr lists for that name). Two copies with the same ID are the
//! same media for Media Composer, even when renamed. Files without any ID are compared by name + size and then by a
//! SHA-256 of their content (only those candidates are read completely). The same physical file seen twice (same
//! device + inode) is not a duplicate; Mountpoint workspaces (mirrors of a folder on another drive) are left out.
//!
//! Setting aside moves the extra copies into "<drive>/MediaFiles Check Duplicates/<time>/<folder>/" on the same drive
//! (a rename, nothing is copied or deleted), recorded in a journal first so it can be undone. Afterwards the databases
//! of the changed folders have to be rewritten (like rebalancing).
use crate::{key, mxf};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq)]
pub struct Copy {
    pub path: PathBuf,
    pub size: u64,
    /// creation date (ns), modification date where the file system has none
    pub created: u128,
    /// the database of its folder lists this file
    pub in_db: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub copies: Vec<Copy>,
    /// index of the copy that stays
    pub keep: usize,
    /// found by the Media Composer ID (false: name + size + checksum)
    pub by_id: bool,
}

impl Group {
    pub fn name(&self) -> String {
        file_name(&self.copies[self.keep].path)
    }

    /// the copies are not all the same size (same ID, different content): never set aside without a look
    pub fn sizes_differ(&self) -> bool {
        self.copies.iter().any(|c| c.size != self.copies[0].size)
    }

    /// bytes the extra copies take
    pub fn extra(&self) -> u64 {
        self.copies.iter().enumerate().filter(|(i, _)| *i != self.keep).map(|(_, c)| c.size).sum()
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Which copy stays: the one its folder's database knows, then the oldest, then the first path.
fn auto_keep(copies: &[Copy]) -> usize {
    (0..copies.len()).min_by_key(|&i| (!copies[i].in_db, copies[i].created, copies[i].path.clone())).unwrap_or(0)
}

/// Mountpoint mounts a folder of another drive as a volume ("mountpoint@<id>", older "mirror@…"): its files are the
/// same files as on the source drive and must not count as copies.
#[cfg(target_os = "macos")]
pub fn is_mirror(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return false;
    }
    let from = unsafe { std::ffi::CStr::from_ptr(s.f_mntfromname.as_ptr()) }.to_string_lossy();
    from.starts_with("mountpoint@") || from.starts_with("mirror@")
}

#[cfg(not(target_os = "macos"))]
pub fn is_mirror(_: &Path) -> bool {
    false
}

fn sha256(path: &Path) -> io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(h.finalize().into())
}

#[derive(Debug, Default)]
pub struct Found {
    pub groups: Vec<Group>,
    pub files: usize,
    /// Mountpoint workspaces that were left out
    pub mirrors: Vec<PathBuf>,
    /// files that could not be read: (path, error)
    pub unreadable: Vec<(PathBuf, String)>,
}

/// Looks through the MXF folders below `roots` (drives, see `crate::folders`); progress gets (done, total, place).
pub fn find(roots: &[PathBuf], progress: &mut dyn FnMut(usize, usize, &str)) -> Found {
    let mut out = Found::default();
    let mut folders = vec![];
    for r in roots {
        if is_mirror(r) {
            out.mirrors.push(r.clone());
        } else {
            folders.extend(crate::folders(r));
        }
    }
    let mut by_id: HashMap<[u8; 32], Vec<Copy>> = HashMap::new();
    let mut by_name: HashMap<(String, u64), Vec<Copy>> = HashMap::new();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    for (i, (place, dir)) in folders.iter().enumerate() {
        progress(i, folders.len(), place);
        // what the folder's database knows: name → ID
        let db: HashMap<String, [u8; 32]> = fs::read(dir.join(crate::PMR))
            .ok()
            .and_then(|d| crate::read_pmr_entries(&d).ok())
            .map(|v| v.into_iter().map(|e| (key(&e.name), e.id)).collect())
            .unwrap_or_default();
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.to_lowercase().ends_with(".mxf") {
                continue;
            }
            let path = e.path();
            let Ok(m) = fs::metadata(&path) else { continue };
            if !m.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if !seen.insert((m.dev(), m.ino())) {
                    continue;
                }
            }
            out.files += 1;
            let created = m.created().or_else(|_| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            let known = db.get(&key(&name)).copied();
            let id = mxf::file_mob_id(&path).ok().or(known);
            let copy = Copy { path, size: m.len(), created, in_db: known.is_some() && known == id };
            match id {
                Some(id) => by_id.entry(id).or_default().push(copy),
                None => by_name.entry((key(&name), m.len())).or_default().push(copy),
            }
        }
    }
    progress(folders.len(), folders.len(), "");
    for copies in by_id.into_values().filter(|c| c.len() > 1) {
        out.groups.push(Group { keep: auto_keep(&copies), copies, by_id: true });
    }
    // no ID: same name + size is only a candidate; the content decides
    for copies in by_name.into_values().filter(|c| c.len() > 1) {
        let mut by_hash: HashMap<[u8; 32], Vec<Copy>> = HashMap::new();
        for c in copies {
            match sha256(&c.path) {
                Ok(h) => by_hash.entry(h).or_default().push(c),
                Err(e) => out.unreadable.push((c.path.clone(), e.to_string())),
            }
        }
        for copies in by_hash.into_values().filter(|c| c.len() > 1) {
            out.groups.push(Group { keep: auto_keep(&copies), copies, by_id: false });
        }
    }
    for g in &mut out.groups {
        g.copies.sort_by(|a, b| a.path.cmp(&b.path));
        g.keep = auto_keep(&g.copies);
    }
    out.groups.sort_by_key(|g| (g.name().to_lowercase(), g.copies[0].path.clone()));
    out
}

/// "<drive>/Avid MediaFiles/MXF/<folder>/<file>" → "<drive>"
fn drive_of(file: &Path) -> Option<&Path> {
    file.parent()?.parent()?.parent()?.parent()
}

/// One "set aside" run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Run {
    /// local time, for the History
    pub date: String,
    /// (where it was, where it is now, size)
    pub moved: Vec<(PathBuf, PathBuf, u64)>,
    /// folders whose databases have to be rewritten
    pub changed: Vec<PathBuf>,
    pub db_done: bool,
    /// folders left with nothing but their database: (where it was, where its contents are now)
    #[serde(default)]
    pub leftovers: Vec<(PathBuf, PathBuf)>,
}

impl Run {
    pub fn bytes(&self) -> u64 {
        self.moved.iter().map(|m| m.2).sum()
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Journal {
    pub entries: Vec<Run>,
}

/// A missing file is the default; an unreadable one is an error – it must never be overwritten blindly.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e),
    }
}

/// Writes next to the file first, then renames: a crash never leaves half a file.
pub fn save<T: Serialize>(value: &T, path: &Path) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(value)?)?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

#[derive(Debug, Default, PartialEq)]
pub struct Moved {
    pub moved: usize,
    /// files left where they are (gone, changed size, or the target name exists)
    pub skipped: Vec<String>,
}

/// Moves these copies into "<drive>/MediaFiles Check Duplicates/<time>/<folder>/". The run is saved in the journal
/// before the first file moves; a copy that is gone or changed in size meanwhile is skipped.
pub fn set_aside(copies: &[Copy], journal_path: &Path) -> io::Result<(Run, Moved)> {
    let stamp = chrono::Local::now();
    let mut run = Run { date: stamp.format("%Y-%m-%d %H:%M").to_string(), moved: vec![], changed: vec![], db_done: false, leftovers: vec![] };
    for c in copies {
        let (Some(drive), Some(folder)) = (drive_of(&c.path), c.path.parent()) else { continue };
        let to = drive.join("MediaFiles Check Duplicates").join(stamp.format("%Y-%m-%d %H%M%S").to_string()).join(folder.file_name().unwrap_or_default()).join(c.path.file_name().unwrap_or_default());
        run.moved.push((c.path.clone(), to, c.size));
        if !run.changed.contains(&folder.to_path_buf()) {
            run.changed.push(folder.to_path_buf());
        }
    }
    let mut journal: Journal = load(journal_path)?;
    journal.entries.push(run.clone());
    save(&journal, journal_path)?;
    let mut out = Moved::default();
    for (from, to, size) in &run.moved {
        if to.exists() || fs::metadata(from).map(|m| m.len()).ok() != Some(*size) {
            out.skipped.push(file_name(from));
            continue;
        }
        fs::create_dir_all(to.parent().unwrap_or(Path::new(".")))?;
        fs::rename(from, to)?;
        out.moved += 1;
    }
    // a folder now without media files goes aside with its database, next to its copies (recorded first, for undo)
    run.leftovers = run.changed.iter().filter(|f| crate::rebalance::is_leftover(f)).filter_map(|f| run.moved.iter().find(|m| m.0.parent() == Some(f.as_path())).map(|m| (f.clone(), m.1.parent().unwrap_or(Path::new(".")).to_path_buf()))).collect();
    if !run.leftovers.is_empty() {
        let mut journal: Journal = load(journal_path)?;
        if let Some(last) = journal.entries.iter_mut().rev().find(|r| r.date == run.date && r.moved == run.moved) {
            last.leftovers = run.leftovers.clone();
        }
        save(&journal, journal_path)?;
        for (folder, into) in &run.leftovers {
            crate::rebalance::move_leftover(folder, into)?;
        }
    }
    Ok((run, out))
}

/// Moves the copies of a run back where they were (only unchanged ones, never over an existing file) and removes
/// the then empty folders below "MediaFiles Check Duplicates".
pub fn undo(run: &Run) -> io::Result<Moved> {
    let mut out = Moved::default();
    let mut dirs: Vec<PathBuf> = vec![];
    // folders that went aside with their database come back first, with it
    for (folder, from) in &run.leftovers {
        let Ok(rd) = fs::read_dir(from) else { continue };
        fs::create_dir_all(folder)?;
        for e in rd.flatten() {
            let to = folder.join(e.file_name());
            if crate::rebalance::is_leftover_entry(&e.path()) && !to.exists() {
                fs::rename(e.path(), to)?;
            }
        }
        dirs.push(from.clone());
    }
    for (from, to, size) in &run.moved {
        if from.exists() || !from.parent().is_some_and(Path::is_dir) || fs::metadata(to).map(|m| m.len()).ok() != Some(*size) {
            out.skipped.push(file_name(from));
            continue;
        }
        fs::rename(to, from)?;
        out.moved += 1;
        dirs.extend(to.parent().map(Path::to_path_buf));
    }
    for d in &dirs {
        // folder of the file, its run folder, "MediaFiles Check Duplicates": each only if empty now
        let mut dir = Some(d.as_path());
        for _ in 0..3 {
            let Some(d) = dir else { break };
            if fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
    Ok(out)
}

/// Text for "Save report…".
pub fn report_text(found: &Found, lang: &str) -> String {
    let t = |a: [&'static str; 4]| crate::pick(lang, a);
    let mut s = format!(
        "{}\n{}\n\n",
        t(["MediaFiles Check – duplicates", "MediaFiles Check – Duplikate", "MediaFiles Check – doublons", "MediaFiles Check – duplicados"]),
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    );
    s += &t(["{g} media files are there more than once ({f} files looked at).\n", "{g} Mediendateien gibt es mehrfach ({f} Dateien angesehen).\n", "{g} fichiers média existent en plusieurs exemplaires ({f} fichiers examinés).\n", "{g} archivos de medios están más de una vez ({f} archivos revisados).\n"])
        .replace("{g}", &found.groups.len().to_string())
        .replace("{f}", &found.files.to_string());
    for g in &found.groups {
        s += &format!("\n{}{}\n", g.name(), if g.by_id { "" } else { t([" (name + size + checksum)", " (Name + Größe + Prüfsumme)", " (nom + taille + somme de contrôle)", " (nombre + tamaño + suma de control)"]) });
        for (i, c) in g.copies.iter().enumerate() {
            let tag = if i == g.keep { t(["stays", "bleibt", "reste", "se queda"]) } else { t(["copy", "Kopie", "copie", "copia"]) };
            s += &format!("  {tag}: {} ({} bytes{})\n", c.path.display(), c.size, if c.in_db { "" } else { t([", not in the database", ", nicht in der Datenbank", ", absent de la base", ", no está en la base de datos"]) });
        }
    }
    for m in &found.mirrors {
        s += &format!("\n{}: {}", t(["Left out (Mountpoint workspace)", "Ausgelassen (Mountpoint-Arbeitsbereich)", "Ignoré (espace Mountpoint)", "Omitido (espacio de Mountpoint)"]), m.display());
    }
    for (p, e) in &found.unreadable {
        s += &format!("\n{}: {} – {e}", t(["Not readable", "Nicht lesbar", "Illisible", "No legible"]), p.display());
    }
    s + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mxf_with_id(n: u8) -> Vec<u8> {
        let mut umid = [0u8; 32];
        umid[..12].copy_from_slice(&[6, 0x0a, 0x2b, 0x34, 1, 1, 1, 1, 1, 1, 0x0f, 0]);
        umid[12] = 0x13;
        umid[16..24].copy_from_slice(&[n, 2, 3, 4, 5, 6, 7, 8]);
        crate::selftest::sample_mxf(&umid)
    }

    #[test]
    fn finds_copies_by_id_and_by_content_and_sets_them_aside() {
        let root = std::env::temp_dir().join(format!("mfc-dups-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (a, b) = (root.join("A"), root.join("B"));
        for d in [&a, &b] {
            fs::create_dir_all(d.join("Avid MediaFiles/MXF/1")).unwrap();
        }
        let (fa, fb) = (a.join("Avid MediaFiles/MXF/1"), b.join("Avid MediaFiles/MXF/1"));
        fs::write(fa.join("clip.mxf"), mxf_with_id(1)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(fb.join("renamed.mxf"), mxf_with_id(1)).unwrap(); // same ID, other name: a copy
        fs::write(fb.join("other.mxf"), mxf_with_id(2)).unwrap(); // other ID: not a copy
        fs::write(fa.join("plain.mxf"), b"no header here").unwrap(); // no ID: name + size + content
        fs::write(fb.join("plain.mxf"), b"no header here").unwrap();
        fs::write(fa.join("same.mxf"), b"aaaa").unwrap(); // same name + size, other content: not a copy
        fs::write(fb.join("same.mxf"), b"bbbb").unwrap();
        let found = find(&[a.clone(), b.clone()], &mut |_, _, _| {});
        assert_eq!(found.files, 7);
        assert_eq!(found.groups.len(), 2);
        let by_id = found.groups.iter().find(|g| g.by_id).unwrap();
        assert_eq!(by_id.copies.len(), 2);
        assert_eq!(by_id.name(), "clip.mxf", "the older copy stays");
        assert!(found.groups.iter().any(|g| !g.by_id && g.name() == "plain.mxf"));
        assert!(report_text(&found, "de").contains("bleibt"));

        let extra: Vec<Copy> = found.groups.iter().flat_map(|g| g.copies.iter().enumerate().filter(|(i, _)| *i != g.keep).map(|(_, c)| c.clone())).collect();
        let jp = root.join("duplicates.json");
        let (run, m) = set_aside(&extra, &jp).unwrap();
        assert_eq!(m.moved, 2);
        assert!(!fb.join("renamed.mxf").exists() && run.moved.iter().all(|r| r.1.exists()));
        assert!(run.moved[0].1.starts_with(b.join("MediaFiles Check Duplicates")), "on the same drive");
        assert_eq!(load::<Journal>(&jp).unwrap().entries, vec![run.clone()]);
        assert_eq!(run.changed, vec![fb.clone()]);
        assert!(find(&[a.clone(), b.clone()], &mut |_, _, _| {}).groups.is_empty());

        // undo: back, never over a file that is there again
        fs::write(fb.join("plain.mxf"), b"new one").unwrap();
        let u = undo(&run).unwrap();
        assert_eq!((u.moved, u.skipped.as_slice()), (1, &["plain.mxf".to_string()][..]));
        assert!(fb.join("renamed.mxf").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_folder_left_with_only_its_database_goes_aside_too() {
        let root = std::env::temp_dir().join(format!("mfc-dups-left-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let folder = root.join("B/Avid MediaFiles/MXF/2");
        fs::create_dir_all(folder.join("Creating")).unwrap();
        fs::write(folder.join("clip.mxf"), b"copy").unwrap();
        fs::write(folder.join(crate::PMR), b"db").unwrap();
        fs::write(folder.join(crate::MDB), b"db").unwrap();
        let copy = Copy { path: folder.join("clip.mxf"), size: 4, created: 0, in_db: false };
        let jp = root.join("duplicates.json");
        let (run, m) = set_aside(&[copy], &jp).unwrap();
        assert_eq!(m.moved, 1);
        assert!(!folder.exists(), "folder went aside");
        let into = run.moved[0].1.parent().unwrap();
        assert!(into.join(crate::PMR).exists() && into.join("Creating").is_dir());
        assert_eq!(load::<Journal>(&jp).unwrap().entries[0].leftovers, vec![(folder.clone(), into.to_path_buf())]);
        let u = undo(&load::<Journal>(&jp).unwrap().entries[0]).unwrap();
        assert_eq!(u.moved, 1);
        assert!(folder.join("clip.mxf").exists() && folder.join(crate::MDB).exists() && folder.join("Creating").is_dir());
        assert!(!root.join("B/MediaFiles Check Duplicates").exists(), "nothing left behind");
        fs::remove_dir_all(&root).unwrap();
    }
}
