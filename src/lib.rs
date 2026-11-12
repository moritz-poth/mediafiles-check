//! MediaFiles Check: compares every Avid MXF folder with its Avid database (msmFMID.pmr).
//!
//! msmFMID.pmr is undocumented; the layout below was worked out from real databases (Media Composer 2023/2024):
//! - block 1: u32 (1961), u32 (8), u32 count, then per file: 32-byte file mob ID, u16 len + file name,
//!   u16 len + workgroup/project name, 32-byte ID + 4 bytes
//! - block 2: u32 (16), u32 count, then the same records with a u32 (name length + 2) in front of the name
//! Block 1 names are Mac Roman and miss names that do not fit it; block 2 names are UTF-8 and complete (see read_pmr).
//! The file must end exactly after block 2; anything else counts as unreadable, so an unknown format never produces
//! a wrong report.
pub mod avb;
pub mod duplicates;
pub mod inventory;
pub mod ferry;
pub mod log;
pub mod media_moves;
pub mod mxf;
pub mod projects;
pub mod rebalance;
pub mod rename;
pub mod report;
pub mod selftest;
pub mod update;

/// This app on GitHub and in its Info.plist (the key is written by scripts/build-mac-app.sh).
pub const UPDATE_APP: update::App = update::App {
    repo: "moritz-poth/mediafiles-check",
    name: "MediaFiles Check",
    bundle_id: "io.github.moritz-poth.mediafiles-check",
    release_key: "MediaFilesCheckRelease",
    // never replace: installed apps trust only zips signed with this key (scripts/update-key.swift, Keychain)
    public_key: "u6xmb0TIVpNgH3B0G2CTddatplyt2R5Lat3BDhvQLPE=",
};

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const PMR: &str = "msmFMID.pmr";
pub const MDB: &str = "msmMMOB.mdb";

/// Moves `from` to `to` but never replaces anything at `to` – not even when it appears between a check and the move
/// (another workstation on a network drive). macOS refuses itself (renamex_np with RENAME_EXCL); where the file system
/// cannot (some network shares), the check right before the move remains. Error AlreadyExists when `to` is taken.
pub fn move_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let taken = || std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists already", to.display()));
    // also first: a file system that ignores RENAME_EXCL (macFUSE without its own support) must not be worse than before
    if to.symlink_metadata().is_ok() {
        return Err(taken());
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(std::io::Error::other);
        let (f, t) = (c(from)?, c(to)?);
        // SAFETY: two valid NUL-terminated paths
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EEXIST) => return Err(taken()),
            // not supported here (macFUSE answers ENOSYS, e.g. Mountpoint workspaces): the check above stays
            Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOSYS) => {}
            _ => return Err(e),
        }
    }
    if to.symlink_metadata().is_ok() {
        return Err(taken());
    }
    fs::rename(from, to)
}

/// Compare key: macOS stores umlauts decomposed (NFD), Windows composed (NFC) and ignores case.
pub fn key(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// little-endian (Intel and Apple silicon); databases from PowerPC Macs are big-endian
    le: bool,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("file ends too early")?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<usize, String> {
        let b: [u8; 2] = self.take(2)?.try_into().unwrap();
        Ok(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) } as usize)
    }
    fn u32(&mut self) -> Result<usize, String> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) } as usize)
    }
    /// 16-bit length + bytes; 0xFFFF is an empty (null) text
    fn counted(&mut self) -> Result<&'a [u8], String> {
        match self.u16()? {
            0xFFFF => Ok(&[]),
            n => self.take(n),
        }
    }
}

/// File names listed in a msmFMID.pmr.
/// Layout: magic 1961, version, count, then one record per file (mob ID, name, project, master mob ID, a modification
/// word). Since version 8 the IDs have 32 bytes, before that 8. The names of this first list are Mac Roman and leave
/// out names it cannot represent (e.g. Turkish "İ"); newer Media Composers add a second list (version 16) with every
/// name in UTF-8, which then counts – the first is only a plausibility check: not longer, and its plain-ASCII names
/// must be in the second. Older databases have only the first list; if the second one is unusable, the first is used.
pub fn read_pmr(data: &[u8]) -> Result<Vec<String>, String> {
    Ok(read_pmr_entries(data)?.into_iter().map(|e| e.name).collect())
}

/// One media file in a msmFMID.pmr.
#[derive(Debug, Clone)]
pub struct PmrEntry {
    pub name: String,
    /// mob ID of the media file (same layout as in bins and MXF headers)
    pub id: [u8; 32],
    /// second mob ID of the record (the master clip)
    pub id2: [u8; 32],
    /// project the file was made in (may be empty)
    pub project: String,
}

const PMR_MAGIC: usize = 1961;
const PMR_UNICODE: usize = 16;

struct PmrRaw<'a> {
    id: &'a [u8],
    name: &'a [u8],
    project: &'a [u8],
    id2: &'a [u8],
}

/// Frames one list without decoding it.
fn pmr_list<'a>(r: &mut Reader<'a>, version: usize, count: usize) -> Result<Vec<PmrRaw<'a>>, String> {
    let id_len = if version <= 7 { 8 } else { 32 };
    let smallest = if version == 1 { 14 } else if version <= 7 { 24 } else { 72 };
    if count > (r.data.len() - r.pos) / smallest {
        return Err(format!("{count} entries announced, the file is too short for them"));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let id = r.take(id_len)?;
        let name = r.counted()?;
        let (project, id2) = if version == 1 { (&[][..], &[][..]) } else { (r.counted()?, r.take(id_len)?) };
        r.take(4)?;
        out.push(PmrRaw { id, name, project, id2 });
    }
    Ok(out)
}

/// A mob ID in the layout bins and MXF headers use: the material fields (32/16/16 bit) little-endian; the old 8-byte
/// IDs are wrapped the way Media Composer wraps them.
fn pmr_mob_id(raw: &[u8], le: bool) -> [u8; 32] {
    let mut id = [0u8; 32];
    match raw.len() {
        32 => {
            id.copy_from_slice(raw);
            if !le {
                id[16..20].reverse();
                id[20..22].reverse();
                id[22..24].reverse();
            }
        }
        8 => {
            id[..16].copy_from_slice(&[6, 0x0a, 0x2b, 0x34, 1, 1, 1, 1, 1, 1, 0x0f, 0, 0x13, 0, 0, 0]);
            id[16..24].copy_from_slice(raw);
            if !le {
                id[16..20].reverse();
                id[20..24].reverse();
            }
            id[24..].copy_from_slice(&[6, 0x0e, 0x2b, 0x34, 0x7f, 0x7f, 0x2a, 0x80]);
        }
        _ => {}
    }
    id
}

fn cut_nul(b: &[u8]) -> Vec<u8> {
    b.iter().position(|&c| c == 0).map_or(b, |i| &b[..i]).to_vec()
}

fn pmr_decode(list: &[PmrRaw], version: usize, le: bool) -> Result<Vec<PmrEntry>, String> {
    list.iter()
        .map(|e| {
            let name = if version == PMR_UNICODE {
                // two bytes before the UTF-8 name, the first one 0
                if e.name.len() < 2 || e.name[0] != 0 {
                    return Err("unexpected name length".to_string());
                }
                String::from_utf8(cut_nul(&e.name[2..])).map_err(|_| "a name is not valid UTF-8")?
            } else {
                avb::mac_roman(&cut_nul(e.name))
            };
            if name.is_empty() {
                return Err("a name is empty".into());
            }
            Ok(PmrEntry { name, id: pmr_mob_id(e.id, le), id2: pmr_mob_id(e.id2, le), project: avb::mac_roman(&cut_nul(e.project)) })
        })
        .collect()
}

/// Names and mob IDs of the media files in a msmFMID.pmr (see read_pmr for the lists).
pub fn read_pmr_entries(data: &[u8]) -> Result<Vec<PmrEntry>, String> {
    let head: [u8; 4] = data.get(..4).ok_or("file ends too early")?.try_into().unwrap();
    let le = if u32::from_le_bytes(head) as usize == PMR_MAGIC {
        true
    } else if u32::from_be_bytes(head) as usize == PMR_MAGIC {
        false
    } else {
        return Err("not an Avid database (unknown beginning)".into());
    };
    let mut r = Reader { data, pos: 4, le };
    let (version, count) = (r.u32()?, r.u32()?);
    if !(1..=8).contains(&version) {
        return Err(format!("unknown database version {version}"));
    }
    let first = pmr_list(&mut r, version, count)?;
    if r.pos == data.len() {
        return pmr_decode(&first, version, le);
    }
    let extension = r.u32()?;
    if extension != PMR_UNICODE {
        return Err(format!("unknown second list (version {extension})"));
    }
    let count2 = r.u32()?;
    let second = pmr_list(&mut r, PMR_UNICODE, count2)?;
    if r.pos != data.len() {
        return Err(format!("{} unexpected bytes at the end", data.len() - r.pos));
    }
    let Ok(entries) = pmr_decode(&second, PMR_UNICODE, le) else {
        return pmr_decode(&first, version, le);
    };
    if count2 < count {
        return Err(format!("the second list is shorter than the first ({count2} / {count})"));
    }
    let all: HashSet<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    if first.iter().any(|e| e.name.is_ascii() && !all.contains(String::from_utf8_lossy(&cut_nul(e.name)).as_ref())) {
        return Err("the two lists differ".into());
    }
    Ok(entries)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Finding {
    /// MXF files but neither database file
    NoDatabase,
    /// one of the two database files is missing
    MissingDbFile(&'static str),
    /// msmFMID.pmr has 0 bytes
    EmptyDatabase,
    Unreadable(String),
    /// MXF files in the folder that the database does not list
    NotInDatabase(Vec<String>),
    /// database entries without a file in the folder
    NoFile(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct FolderReport {
    /// "Drive/Folder"
    pub place: String,
    pub path: PathBuf,
    pub files: usize,
    pub findings: Vec<Finding>,
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Compares one MXF folder with its database.
pub fn check_folder(dir: &Path, place: String) -> FolderReport {
    let mut report = FolderReport { place, path: dir.to_path_buf(), files: 0, findings: vec![] };
    let entries = match fs::read_dir(dir).and_then(|rd| rd.map(|e| e.map(|e| e.path())).collect::<Result<Vec<_>, _>>()) {
        Ok(e) => e,
        Err(e) => {
            report.findings.push(Finding::Unreadable(e.to_string()));
            return report;
        }
    };
    let (mut files, mut pmr, mut mdb) = (vec![], None, false);
    for p in entries {
        let n = name_of(&p);
        if n.starts_with('.') || p.is_dir() {
            continue; // includes Avid's "Creating" subfolder
        }
        if n.eq_ignore_ascii_case(PMR) {
            pmr = Some(p);
        } else if n.eq_ignore_ascii_case(MDB) {
            mdb = true;
        } else if n.to_lowercase().ends_with(".mxf") {
            files.push(n);
        }
    }
    report.files = files.len();
    let f = &mut report.findings;
    match (&pmr, mdb) {
        (None, false) => {
            if !files.is_empty() {
                f.push(Finding::NoDatabase);
            }
            return report;
        }
        (None, true) => {
            f.push(Finding::MissingDbFile(PMR));
            return report;
        }
        (Some(_), false) => f.push(Finding::MissingDbFile(MDB)),
        _ => {}
    }
    let names = match fs::read(pmr.unwrap()) {
        Ok(d) if d.is_empty() => {
            f.push(Finding::EmptyDatabase);
            return report;
        }
        Ok(d) => read_pmr(&d),
        Err(e) => Err(e.to_string()),
    };
    let names = match names {
        Ok(n) => n,
        Err(e) => {
            f.push(Finding::Unreadable(e));
            return report;
        }
    };
    let in_db: HashSet<String> = names.iter().map(|n| key(n)).collect();
    let on_disk: HashSet<String> = files.iter().map(|n| key(n)).collect();
    let mut not_in_db: Vec<String> = files.into_iter().filter(|n| !in_db.contains(&key(n))).collect();
    let mut no_file: Vec<String> = names.into_iter().filter(|n| !on_disk.contains(&key(n))).collect();
    not_in_db.sort();
    no_file.sort();
    if !not_in_db.is_empty() {
        f.push(Finding::NotInDatabase(not_in_db));
    }
    if !no_file.is_empty() {
        f.push(Finding::NoFile(no_file));
    }
    report
}

pub fn mxf_dir(volume: &Path) -> PathBuf {
    volume.join("Avid MediaFiles").join("MXF")
}

/// All mounted drives (network drives too) with "Avid MediaFiles/MXF".
pub fn find_volumes() -> Vec<PathBuf> {
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        (b'A'..=b'Z').map(|c| PathBuf::from(format!("{}:\\", c as char))).collect()
    } else {
        fs::read_dir("/Volumes").map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default()
    };
    // test lab (scripts/testlab.sh): only these drive names, so tests never see real drives
    let only: Option<Vec<String>> = std::env::var("MFC_LAB_DRIVES").ok().map(|v| v.split(',').map(str::to_string).collect());
    let mut found: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|p| mxf_dir(p).is_dir() && only.as_ref().is_none_or(|o| o.iter().any(|n| p.file_name().is_some_and(|f| f.to_string_lossy() == *n))))
        .collect();
    found.sort();
    found
}

/// The test lab's stand-in computer name (MFC_LAB_COMPUTER), so two "systems" can run on one Mac.
pub fn lab_computer() -> Option<String> {
    std::env::var("MFC_LAB_COMPUTER").ok().filter(|n| !n.is_empty())
}

/// A recorded folder whose drive is mounted under another path now (after a crash macOS may mount "Media" as
/// "/Volumes/Media 1"; a drive can be renamed): the MXF folder on another drive that holds `folder` with every
/// recorded MXF file. Media Composer gives each MXF file a unique name, so the names identify the folder; a folder
/// without recorded files is never matched. None while `mxf` itself still exists.
pub fn find_moved(mxf: &Path, folder: &str, files: &[(String, u64)]) -> Option<PathBuf> {
    if files.is_empty() || mxf.is_dir() {
        return None;
    }
    find_volumes().into_iter().map(|v| mxf_dir(&v)).filter(|m| m != mxf).find(|m| {
        let have: std::collections::HashSet<String> = fs::read_dir(m.join(folder)).map(|rd| rd.flatten().map(|e| key(&e.file_name().to_string_lossy())).collect()).unwrap_or_default();
        files.iter().all(|(n, _)| have.contains(&key(n)))
    })
}

fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !name_of(p).starts_with('.')).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn is_media_folder(dir: &Path) -> bool {
    fs::read_dir(dir).map(|rd| rd.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().to_lowercase();
        n.ends_with(".mxf") || n == PMR.to_lowercase() || n == MDB.to_lowercase()
    })).unwrap_or(false)
}

/// The MXF folders below a chosen place: a drive (with Avid MediaFiles/MXF), one MXF folder, or a folder of MXF folders.
pub fn folders(root: &Path) -> Vec<(String, PathBuf)> {
    let label = name_of(root);
    let dirs = if mxf_dir(root).is_dir() {
        subfolders(&mxf_dir(root))
    } else if is_media_folder(root) {
        return vec![(label, root.to_path_buf())];
    } else {
        subfolders(root)
    };
    dirs.into_iter().map(|d| (format!("{label}/{}", name_of(&d)), d)).collect()
}

/// Checks every MXF folder below the given places; progress gets (done, total, place).
pub fn check(roots: &[PathBuf], progress: &mut dyn FnMut(usize, usize, &str)) -> Vec<FolderReport> {
    let all: Vec<(String, PathBuf)> = roots.iter().flat_map(|r| folders(r)).collect();
    all.iter()
        .enumerate()
        .map(|(i, (place, dir))| {
            progress(i, all.len(), place);
            check_folder(dir, place.clone())
        })
        .collect()
}

/// Picks the text for a language code (en, de, fr, es; anything else = English).
pub fn pick<'a>(lang: &str, [en, de, fr, es]: [&'a str; 4]) -> &'a str {
    match lang {
        "de" => de,
        "fr" => fr,
        "es" => es,
        _ => en,
    }
}

/// "{n} <singular|plural>" – the plural form is picked by n.
fn count(lang: &str, n: usize, one: [&str; 4], many: [&str; 4]) -> String {
    format!("{n} {}", pick(lang, if n == 1 { one } else { many }))
}

const MXF_FILE: [&str; 4] = ["MXF file", "MXF-Datei", "fichier MXF", "archivo MXF"];
const MXF_FILES: [&str; 4] = ["MXF files", "MXF-Dateien", "fichiers MXF", "archivos MXF"];

impl Finding {
    /// One line in the given language (en, de, fr, es), without the file names.
    pub fn describe(&self, files: usize, lang: &str) -> String {
        let t = |texts: [&str; 4]| pick(lang, texts).to_string();
        match self {
            Finding::NoDatabase => t(["no database ({})", "keine Datenbank ({})", "pas de base de données ({})", "sin base de datos ({})"])
                .replace("{}", &count(lang, files, MXF_FILE, MXF_FILES)),
            Finding::MissingDbFile(name) => t(["{} is missing", "{} fehlt", "{} manque", "falta {}"]).replace("{}", name),
            Finding::EmptyDatabase => t(["database is empty ({})", "Datenbank ist leer ({})", "base de données vide ({})", "base de datos vacía ({})"])
                .replace("{}", &count(lang, files, MXF_FILE, MXF_FILES)),
            Finding::Unreadable(e) => t(["database unreadable: {}", "Datenbank nicht lesbar: {}", "base de données illisible : {}", "base de datos ilegible: {}"]).replace("{}", e),
            Finding::NotInDatabase(n) => t(["{} not in the database", "{} nicht in der Datenbank", "{} absent(s) de la base de données", "{} no está(n) en la base de datos"])
                .replace("{}", &count(lang, n.len(), ["file", "Datei", "fichier", "archivo"], ["files", "Dateien", "fichiers", "archivos"])),
            Finding::NoFile(n) => t(["{} in the database without a file", "{} in der Datenbank ohne Datei", "{} dans la base de données sans fichier", "{} en la base de datos sin archivo"])
                .replace("{}", &count(lang, n.len(), ["entry", "Eintrag", "entrée", "entrada"], ["entries", "Einträge", "entrées", "entradas"])),
        }
    }

    pub fn names(&self) -> &[String] {
        match self {
            Finding::NotInDatabase(n) | Finding::NoFile(n) => n,
            _ => &[],
        }
    }
}

/// Full report as text (for saving): every folder with findings and every file name.
pub fn report_text(reports: &[FolderReport], lang: &str) -> String {
    let bad: Vec<&FolderReport> = reports.iter().filter(|r| !r.findings.is_empty()).collect();
    let head = pick(lang, [
        "MediaFiles Check – {n} folders checked, {b} with differences",
        "MediaFiles Check – {n} Ordner geprüft, {b} mit Abweichungen",
        "MediaFiles Check – {n} dossiers vérifiés, {b} avec des différences",
        "MediaFiles Check – {n} carpetas comprobadas, {b} con diferencias",
    ]);
    let mut out = head.replace("{n}", &reports.len().to_string()).replace("{b}", &bad.len().to_string()) + "\n";
    for r in bad {
        out.push_str(&format!("\n{}\n", r.place));
        for f in &r.findings {
            out.push_str(&format!("  {}\n", f.describe(r.files, lang)));
            for n in f.names() {
                out.push_str(&format!("    {n}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::selftest::sample_pmr as pmr;

    #[test]
    fn reads_both_blocks_and_rejects_anything_else() {
        let d = pmr(&["a.mxf", "b.mxf"]);
        assert_eq!(read_pmr(&d).unwrap(), ["a.mxf", "b.mxf"]);
        // umlauts and characters Mac Roman lacks: block 2 (UTF-8) is complete
        let intl = pmr(&["Kölsch.mxf", "İdo Tatlıs.mxf", "c.mxf"]);
        assert_eq!(read_pmr(&intl).unwrap(), ["Kölsch.mxf", "İdo Tatlıs.mxf", "c.mxf"]);
        assert!(read_pmr(&d[..d.len() - 1]).is_err());
        let mut longer = d.clone();
        longer.push(0);
        assert!(read_pmr(&longer).is_err());
        let mut changed = d.clone();
        let i = changed.len() - 50; // inside the last name of block 2
        changed[i] = b'X';
        assert!(read_pmr(&changed).is_err());
    }

    /// A database with only the first list, as older Media Composers write it.
    fn old_pmr(version: u32, names: &[&[u8]], le: bool) -> Vec<u8> {
        let n32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let n16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let id_len = if version <= 7 { 8 } else { 32 };
        let mut d = vec![];
        d.extend(n32(1961));
        d.extend(n32(version));
        d.extend(n32(names.len() as u32));
        for (i, n) in names.iter().enumerate() {
            let mut id = vec![0u8; id_len];
            let at = if id_len == 32 { 16 } else { 0 };
            id[at..at + 4].copy_from_slice(&n32(0x0102_0300 + i as u32));
            d.extend(&id);
            d.extend(n16(n.len() as u16));
            d.extend(*n);
            d.extend(n16(0xFFFF)); // no project
            d.extend(&id);
            d.extend([0; 4]);
        }
        d
    }

    #[test]
    fn reads_older_databases() {
        // version 7: 8-byte IDs, only the Mac Roman list ("\x9a" = ö), both byte orders give the same IDs
        let le = read_pmr_entries(&old_pmr(7, &[b"K\x9alsch.mxf", b"b.mxf"], true)).unwrap();
        let be = read_pmr_entries(&old_pmr(7, &[b"K\x9alsch.mxf", b"b.mxf"], false)).unwrap();
        assert_eq!(le.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Kölsch.mxf", "b.mxf"]);
        assert_eq!(le[1].id, be[1].id);
        assert_eq!(&le[1].id[..4], &[6, 0x0a, 0x2b, 0x34]);
        assert_eq!(&le[1].id[16..20], &0x0102_0301u32.to_le_bytes());
        // version 8 big-endian: the material fields are turned around
        let be8 = read_pmr_entries(&old_pmr(8, &[b"a.mxf"], false)).unwrap();
        assert_eq!(&be8[0].id[16..20], &0x0102_0300u32.to_le_bytes());
        // a name cut by a 0 byte, an unknown beginning, an unknown version
        assert_eq!(read_pmr(&old_pmr(8, &[b"c.mxf\0\0"], true)).unwrap(), ["c.mxf"]);
        assert!(read_pmr(&[0, 0, 0, 0]).is_err());
        assert!(read_pmr(&old_pmr(9, &[b"a.mxf"], true)).is_err());
    }

    #[test]
    fn broken_utf8_list_falls_back_to_the_first() {
        let mut d = pmr(&["a.mxf", "b.mxf"]);
        let i = d.len() - 50; // inside the last name of the UTF-8 list
        d[i] = 0xff;
        assert_eq!(read_pmr(&d).unwrap(), ["a.mxf", "b.mxf"]);
    }

    #[test]
    fn move_new_never_replaces() {
        let dir = std::env::temp_dir().join(format!("mfc-move-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("empty")).unwrap();
        fs::write(dir.join("a"), "a").unwrap();
        fs::write(dir.join("b"), "b").unwrap();
        fs::create_dir(dir.join("d")).unwrap();
        assert_eq!(move_new(&dir.join("a"), &dir.join("b")).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(dir.join("b")).unwrap(), "b");
        // rename() would replace an empty folder
        assert!(move_new(&dir.join("d"), &dir.join("empty")).is_err());
        move_new(&dir.join("a"), &dir.join("c")).unwrap();
        assert_eq!(fs::read_to_string(dir.join("c")).unwrap(), "a");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_is_compared_with_its_database() {
        let dir = std::env::temp_dir().join(format!("mediafiles-check-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("1/Creating")).unwrap();
        for f in ["a.mxf", "B.MXF", "new.mxf", ".hidden.mxf", "notes.txt"] {
            fs::write(dir.join("1").join(f), b"").unwrap();
        }
        fs::write(dir.join("1").join(PMR), pmr(&["a.mxf", "b.mxf", "gone.mxf"])).unwrap();
        fs::write(dir.join("1").join(MDB), b"x").unwrap();
        let r = check_folder(&dir.join("1"), "V/1".into());
        assert_eq!(r.files, 3);
        assert_eq!(r.findings, [Finding::NotInDatabase(vec!["new.mxf".into()]), Finding::NoFile(vec!["gone.mxf".into()])]);

        fs::create_dir_all(dir.join("2")).unwrap();
        fs::write(dir.join("2/x.mxf"), b"").unwrap();
        assert_eq!(check_folder(&dir.join("2"), "V/2".into()).findings, [Finding::NoDatabase]);
        fs::write(dir.join("2").join(PMR), b"").unwrap();
        assert_eq!(check_folder(&dir.join("2"), "V/2".into()).findings, [Finding::MissingDbFile(MDB), Finding::EmptyDatabase]);

        fs::create_dir_all(dir.join("3")).unwrap(); // empty folder: nothing to report
        assert!(check_folder(&dir.join("3"), "V/3".into()).findings.is_empty());

        let places: Vec<String> = folders(&dir).into_iter().map(|f| f.0).collect();
        assert_eq!(places.len(), 3);
        assert!(report_text(&check(&[dir.clone()], &mut |_, _, _| {}), "de").contains("1 Datei nicht in der Datenbank"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
