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
pub mod log;
pub mod mxf;
pub mod projects;
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

/// Compare key: macOS stores umlauts decomposed (NFD), Windows composed (NFC) and ignores case.
pub fn key(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("file ends too early")?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<usize, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize)
    }
    fn u32(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }
}

/// File names listed in a msmFMID.pmr.
/// Block 1 holds the names in the old single-byte character set (Mac Roman on a Mac) and leaves out names it cannot
/// represent (e.g. Turkish "İ"); block 2 holds every name in UTF-8 and is what counts. Block 1 is only a plausibility
/// check: not longer than block 2, and its plain-ASCII names must be in block 2.
pub fn read_pmr(data: &[u8]) -> Result<Vec<String>, String> {
    Ok(read_pmr_entries(data)?.into_iter().map(|e| e.name).collect())
}

/// One media file in a msmFMID.pmr.
#[derive(Debug, Clone)]
pub struct PmrEntry {
    pub name: String,
    /// mob ID of the media file (same layout as in bins and MXF headers)
    pub id: [u8; 32],
    /// second mob ID of the record
    pub id2: [u8; 32],
}

/// Names and mob IDs of the media files in a msmFMID.pmr (see read_pmr for the two blocks).
pub fn read_pmr_entries(data: &[u8]) -> Result<Vec<PmrEntry>, String> {
    let mut r = Reader { data, pos: 0 };
    let (_, _, count) = (r.u32()?, r.u32()?, r.u32()?);
    let mut ascii = vec![];
    for _ in 0..count {
        r.take(32)?;
        let len = r.u16()?;
        let n = r.take(len)?;
        if n.is_ascii() {
            ascii.push(String::from_utf8_lossy(n).into_owned());
        }
        let len = r.u16()?;
        r.take(len + 36)?;
    }
    let (_, count2) = (r.u32()?, r.u32()?);
    if count2 < count {
        return Err(format!("the second list is shorter than the first ({count2} / {count})"));
    }
    let mut entries = Vec::with_capacity(count2.min(100_000));
    for _ in 0..count2 {
        let id: [u8; 32] = r.take(32)?.try_into().unwrap();
        let len = r.u32()?.checked_sub(2).ok_or("unexpected name length")?;
        let name = String::from_utf8(r.take(len)?.to_vec()).map_err(|_| "a name is not valid UTF-8")?;
        let len = r.u16()?;
        r.take(len)?;
        let id2: [u8; 32] = r.take(32)?.try_into().unwrap();
        r.take(4)?;
        entries.push(PmrEntry { name, id, id2 });
    }
    if r.pos != data.len() {
        return Err(format!("{} unexpected bytes at the end", data.len() - r.pos));
    }
    let all: HashSet<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    if ascii.iter().any(|n| !all.contains(n.as_str())) {
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
    let mut found: Vec<PathBuf> = candidates.into_iter().filter(|p| mxf_dir(p).is_dir()).collect();
    found.sort();
    found
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
