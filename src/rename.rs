//! Rename folders so Media Composer rebuilds their databases, and rename them back afterwards.
//!
//! Media Composer only updates the databases of its own folders: numbers on local drives ("1", "2" …),
//! "<computer name>.<number>" on network drives. Before renaming, the original name and every MXF file
//! (name + size) are written to a journal, so the folder can be identified and checked when it is renamed back.
use crate::{MDB, PMR, key};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// (size, modification time in ns) of a database file; None = missing
pub type Stamp = Option<(u64, u128)>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Renamed {
    /// the MXF folder that holds it
    pub mxf: PathBuf,
    pub original: String,
    pub current: String,
    /// MXF files at the time of renaming: (name, size)
    pub files: Vec<(String, u64)>,
    /// msmFMID.pmr, msmMMOB.mdb at the time of renaming
    pub db: [Stamp; 2],
}

impl Renamed {
    pub fn dir(&self) -> PathBuf {
        self.mxf.join(&self.current)
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Journal {
    pub entries: Vec<Renamed>,
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
}

fn stamp(p: &Path) -> Stamp {
    let m = fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((m.len(), t))
}

pub fn db_stamps(dir: &Path) -> [Stamp; 2] {
    [stamp(&dir.join(PMR)), stamp(&dir.join(MDB))]
}

/// MXF files in a folder (name, size), sorted; hidden files are left out.
pub fn mxf_files(dir: &Path) -> io::Result<Vec<(String, u64)>> {
    let mut v = vec![];
    for e in fs::read_dir(dir)? {
        let e = e?;
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with('.') && n.to_lowercase().ends_with(".mxf") && e.file_type()?.is_file() {
            v.push((n, e.metadata()?.len()));
        }
    }
    v.sort();
    Ok(v)
}

/// The name Media Composer uses for its network folders, e.g. "Mac Studio.1".
pub fn computer_name() -> String {
    let out = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok();
    match out.filter(|o| o.status.success()) {
        Some(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        // shortcut: Windows has no scutil; use the COMPUTERNAME variable there when the Windows build comes
        None => std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Computer".into()),
    }
}

/// Whether the folder lies on a network drive.
#[cfg(target_os = "macos")]
pub fn is_network(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    unsafe { libc::statfs(c.as_ptr(), &mut s) == 0 && s.f_flags & libc::MNT_LOCAL as u32 == 0 }
}

/// shortcut: other systems count as local until the Windows build (GetDriveType) comes
#[cfg(not(target_os = "macos"))]
pub fn is_network(_: &Path) -> bool {
    false
}

/// Is this a name Media Composer updates on this system?
pub fn is_avid_name(name: &str, network: bool, computer: &str) -> bool {
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match network {
        false => number(name),
        true => name.rsplit_once('.').is_some_and(|(c, n)| key(c) == key(computer) && number(n)),
    }
}

/// Lies directly in ".../Avid MediaFiles/MXF" – only those are renamed.
pub fn is_in_mxf(folder: &Path) -> bool {
    let name = |p: Option<&Path>| p.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase());
    let mxf = folder.parent();
    name(mxf).as_deref() == Some("mxf") && name(mxf.and_then(|m| m.parent())).as_deref() == Some("avid mediafiles")
}

pub fn needs_rename(folder: &Path, computer: &str) -> bool {
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    is_in_mxf(folder) && !is_avid_name(&name, is_network(folder), computer)
}

fn avid_folder_name(n: usize, network: bool, computer: &str) -> String {
    if network { format!("{computer}.{n}") } else { n.to_string() }
}

/// Smallest free number (or computer.number) in an MXF folder; `reserved` holds keys already planned.
pub fn free_name(mxf: &Path, network: bool, computer: &str, reserved: &HashSet<String>) -> String {
    let taken: HashSet<String> = fs::read_dir(mxf).map(|rd| rd.flatten().map(|e| key(&e.file_name().to_string_lossy())).collect()).unwrap_or_default();
    (1..).map(|n| avid_folder_name(n, network, computer)).find(|c| !taken.contains(&key(c)) && !reserved.contains(&key(c))).unwrap()
}

/// New names for several folders at once (two folders in the same MXF folder never get the same number).
pub fn plan_names(folders: &[PathBuf], computer: &str) -> Vec<String> {
    let mut reserved: Vec<(PathBuf, HashSet<String>)> = vec![];
    folders
        .iter()
        .map(|f| {
            let mxf = f.parent().unwrap_or(Path::new("")).to_path_buf();
            let i = reserved.iter().position(|r| r.0 == mxf).unwrap_or_else(|| {
                reserved.push((mxf.clone(), HashSet::new()));
                reserved.len() - 1
            });
            let name = free_name(&mxf, is_network(f), computer, &reserved[i].1);
            reserved[i].1.insert(key(&name));
            name
        })
        .collect()
}

/// Records the folder in the journal (saved first), then renames it.
pub fn rename_for_avid(folder: &Path, new_name: &str, journal: &mut Journal, journal_path: &Path) -> io::Result<()> {
    let mxf = folder.parent().ok_or_else(|| io::Error::other("no parent folder"))?.to_path_buf();
    let target = mxf.join(new_name);
    if target.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists already", target.display())));
    }
    let original = folder.file_name().unwrap_or_default().to_string_lossy().into_owned();
    journal.entries.push(Renamed { mxf, original, current: new_name.into(), files: mxf_files(folder)?, db: db_stamps(folder) });
    journal.save(journal_path)?;
    if let Err(e) = fs::rename(folder, &target) {
        journal.entries.pop();
        journal.save(journal_path)?;
        return Err(e);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// drive not connected or folder gone
    Offline,
    /// Media Composer has not written the database yet
    Waiting,
    /// both database files exist and changed since renaming
    Ready,
}

pub fn status(r: &Renamed) -> Status {
    let dir = r.dir();
    if !dir.is_dir() {
        return Status::Offline;
    }
    let now = db_stamps(&dir);
    if now.iter().all(Option::is_some) && now != r.db { Status::Ready } else { Status::Waiting }
}

#[derive(Debug, Default, PartialEq)]
pub struct Diff {
    /// MXF files that were not there when the folder was renamed
    pub added: Vec<String>,
    /// files that are gone or have a different size
    pub changed: Vec<String>,
}

pub fn diff(r: &Renamed) -> io::Result<Diff> {
    let files = mxf_files(&r.dir())?;
    let now: std::collections::HashMap<String, u64> = files.iter().map(|(n, size)| (key(n), *size)).collect();
    let before: HashSet<String> = r.files.iter().map(|f| key(&f.0)).collect();
    let added = files.into_iter().map(|f| f.0).filter(|n| !before.contains(&key(n))).collect();
    let changed = r.files.iter().filter(|(n, size)| now.get(&key(n)) != Some(size)).map(|f| f.0.clone()).collect();
    Ok(Diff { added, changed })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Added {
    /// leave new files in the folder
    Keep,
    /// move them to a new numbered folder
    NewFolder,
    /// move them to "1" (local) or "<computer>.1" (network)
    FirstFolder,
}

#[derive(Debug, Default, PartialEq)]
pub struct Back {
    /// folder the new files went to, and how many
    pub moved_to: Option<(String, usize)>,
    /// files not moved because the target already has a file with that name
    pub not_moved: Vec<String>,
}

/// Renames the folder back to its original name; afterwards moves the new files if asked.
/// Never overwrites: an existing folder with the original name or an existing file stops/skips.
pub fn rename_back(r: &Renamed, added: &[String], choice: Added, computer: &str) -> io::Result<Back> {
    let original = r.mxf.join(&r.original);
    if original.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists already", original.display())));
    }
    let network = is_network(&r.dir());
    fs::rename(r.dir(), &original)?;
    let mut back = Back::default();
    if added.is_empty() || choice == Added::Keep {
        return Ok(back);
    }
    let target_name = match choice {
        Added::FirstFolder => avid_folder_name(1, network, computer),
        _ => free_name(&r.mxf, network, computer, &HashSet::new()),
    };
    let target = r.mxf.join(&target_name);
    fs::create_dir_all(&target)?;
    let mut moved = 0;
    for f in added {
        let dst = target.join(f);
        if dst.exists() {
            back.not_moved.push(f.clone());
        } else {
            fs::rename(original.join(f), dst)?;
            moved += 1;
        }
    }
    back.moved_to = Some((target_name, moved));
    Ok(back)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mfc-rename-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mxf = root.join("Avid MediaFiles/MXF");
        fs::create_dir_all(mxf.join("Avid10.1")).unwrap();
        fs::create_dir_all(mxf.join("1")).unwrap();
        fs::write(mxf.join("Avid10.1/a.mxf"), b"aa").unwrap();
        fs::write(mxf.join("Avid10.1/b.mxf"), b"bbb").unwrap();
        fs::write(mxf.join("Avid10.1").join(PMR), b"old").unwrap();
        mxf
    }

    #[test]
    fn names() {
        assert!(is_avid_name("12", false, "Mac"));
        assert!(!is_avid_name("Avid10.1", false, "Mac"));
        assert!(is_avid_name("Mac Studio.3", true, "Mac Studio"));
        assert!(!is_avid_name("Other.3", true, "Mac Studio"));
        assert!(!is_avid_name("3", true, "Mac Studio"));
        assert!(is_in_mxf(Path::new("/V/Avid MediaFiles/MXF/1")));
        assert!(!is_in_mxf(Path::new("/V/Backup/1")));
    }

    #[test]
    fn rename_wait_and_back_with_new_files() {
        let mxf = setup("back");
        let jp = mxf.join("../../journal.json");
        let mut j = Journal::default();
        let folder = mxf.join("Avid10.1");
        let planned = plan_names(&[folder.clone(), mxf.join("x")], "Mac");
        assert_eq!(planned, ["2", "3"]); // "1" exists, two folders never share a number
        rename_for_avid(&folder, "2", &mut j, &jp).unwrap();
        assert!(!folder.exists() && mxf.join("2/a.mxf").exists());
        assert_eq!(Journal::load(&jp).unwrap().entries, j.entries);
        let r = j.entries[0].clone();
        assert_eq!(r.files, [("a.mxf".into(), 2), ("b.mxf".into(), 3)]);
        assert_eq!(status(&r), Status::Waiting);

        // Media Composer writes the database and records a new file
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(mxf.join("2").join(PMR), b"new list").unwrap();
        fs::write(mxf.join("2").join(MDB), b"db").unwrap();
        fs::write(mxf.join("2/new.mxf"), b"n").unwrap();
        assert_eq!(status(&r), Status::Ready);
        let d = diff(&r).unwrap();
        assert_eq!(d, Diff { added: vec!["new.mxf".into()], changed: vec![] });

        let back = rename_back(&r, &d.added, Added::FirstFolder, "Mac").unwrap();
        assert_eq!(back.moved_to, Some(("1".into(), 1)));
        assert!(mxf.join("Avid10.1/a.mxf").exists() && mxf.join("1/new.mxf").exists() && !mxf.join("2").exists());
        fs::remove_dir_all(mxf.join("../..")).unwrap();
    }

    #[test]
    fn never_overwrites() {
        let mxf = setup("safe");
        let jp = mxf.join("../../journal.json");
        let mut j = Journal::default();
        assert!(rename_for_avid(&mxf.join("Avid10.1"), "1", &mut j, &jp).is_err()); // "1" exists
        assert!(j.entries.is_empty() && mxf.join("Avid10.1").exists());
        rename_for_avid(&mxf.join("Avid10.1"), "2", &mut j, &jp).unwrap();
        fs::remove_file(mxf.join("2/b.mxf")).unwrap();
        assert_eq!(diff(&j.entries[0]).unwrap().changed, ["b.mxf"]);
        fs::create_dir_all(mxf.join("Avid10.1")).unwrap(); // someone made a folder with the old name
        assert!(rename_back(&j.entries[0], &[], Added::Keep, "Mac").is_err());
        assert!(mxf.join("2/a.mxf").exists());
        fs::remove_dir_all(mxf.join("../..")).unwrap();
    }
}
