//! Self-test for testers without developer tools (Settings → Support → "Run self-test…"): checks the app's core
//! functions on its own test data in a temporary folder – never on the user's media, folders or rename journal –
//! and builds an anonymous report (src/report.rs). The only things it looks at outside its folder: whether the
//! drives with "Avid MediaFiles" can be read (listing only) and GitHub (update download + signature).
use crate::report::{self, CheckResult, Report};
use crate::{Finding, MDB, PMR, check_folder, find_volumes, mxf, mxf_dir, read_pmr, rename};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub enum Failed {
    Skip(String),
    Fail(String),
}

fn fail<T>(text: impl Into<String>) -> Result<T, Failed> {
    Err(Failed::Fail(text.into()))
}

fn io<T>(r: std::io::Result<T>) -> Result<T, Failed> {
    r.map_err(|e| Failed::Fail(e.to_string()))
}

/// A database the way Media Composer writes it (both blocks, see read_pmr). Also used by the unit tests.
pub fn sample_pmr(names: &[&str]) -> Vec<u8> {
    let mut d = vec![];
    let put = |d: &mut Vec<u8>, b: &[u8]| d.extend_from_slice(b);
    // block 1: "ö" in Mac Roman (0x9A), names with "İ" left out
    let block1: Vec<Vec<u8>> = names.iter().filter(|n| !n.contains('İ')).map(|n| n.replace('ö', "\u{9a}").chars().map(|c| c as u8).collect()).collect();
    put(&mut d, &1961u32.to_le_bytes());
    put(&mut d, &8u32.to_le_bytes());
    put(&mut d, &(block1.len() as u32).to_le_bytes());
    for n in &block1 {
        put(&mut d, &[6; 32]);
        put(&mut d, &(n.len() as u16).to_le_bytes());
        put(&mut d, n);
        put(&mut d, &7u16.to_le_bytes());
        put(&mut d, b"PROJECT");
        put(&mut d, &[1; 36]);
    }
    put(&mut d, &16u32.to_le_bytes());
    put(&mut d, &(names.len() as u32).to_le_bytes());
    for n in names {
        put(&mut d, &[6; 32]);
        put(&mut d, &(n.len() as u32 + 2).to_le_bytes());
        put(&mut d, n.as_bytes());
        put(&mut d, &7u16.to_le_bytes());
        put(&mut d, b"PROJECT");
        put(&mut d, &[1; 36]);
    }
    d
}

/// Header of an MXF file whose essence container data links to `umid`. Also used by the unit tests.
pub fn sample_mxf(umid: &[u8; 32]) -> Vec<u8> {
    let mut d = vec![];
    let mut partition = mxf::PARTITION.to_vec();
    partition.extend([0x02, 0x04, 0x00]);
    d.extend(&partition);
    d.push(88);
    d.extend([0u8; 88]);
    // some other set first (material package), with a long-form BER length
    let mut other = mxf::SET;
    other[14] = 0x36;
    d.extend(other);
    d.extend([0x83, 0, 0, 6, 0x44, 0x01, 0, 2, 9, 9]);
    let mut ecd = mxf::SET;
    ecd[14] = mxf::ESSENCE_CONTAINER_DATA;
    d.extend(ecd);
    d.push(4 + 4 + 32);
    d.extend([0x3f, 0x07, 0, 0]);
    d.extend([0x27, 0x01, 0, 32]);
    d.extend(umid);
    d
}

fn database(_: &Path) -> Result<String, Failed> {
    let names = ["a.mxf", "Kölsch.mxf", "İdo Tatlıs.mxf"];
    let d = sample_pmr(&names);
    if read_pmr(&d).map_err(Failed::Fail)? != names {
        return fail("names read differ from the names written");
    }
    if read_pmr(&d[..d.len() - 1]).is_ok() {
        return fail("a damaged database was read as if it were fine");
    }
    Ok("3 names with umlauts and Turkish letters read; a damaged database is refused".into())
}

fn compare(base: &Path) -> Result<String, Failed> {
    let dir = base.join("compare/1");
    io(fs::create_dir_all(&dir))?;
    for f in ["a.mxf", "new.mxf"] {
        io(fs::write(dir.join(f), b""))?;
    }
    io(fs::write(dir.join(PMR), sample_pmr(&["a.mxf", "gone.mxf"])))?;
    io(fs::write(dir.join(MDB), b"x"))?;
    let r = check_folder(&dir, "Test/1".into());
    let want = [Finding::NotInDatabase(vec!["new.mxf".into()]), Finding::NoFile(vec!["gone.mxf".into()])];
    if r.findings != want {
        return fail(format!("expected one new and one missing file, got {:?}", r.findings));
    }
    Ok("new file and missing file found".into())
}

fn mxf_header(base: &Path) -> Result<String, Failed> {
    let mut umid = [0u8; 32];
    umid[..12].copy_from_slice(&[6, 0x0a, 0x2b, 0x34, 1, 1, 1, 1, 1, 1, 0x0f, 0]);
    umid[12] = 0x13;
    umid[16..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let path = base.join("header.mxf");
    io(fs::write(&path, sample_mxf(&umid)))?;
    let id = mxf::file_mob_id(&path).map_err(Failed::Fail)?;
    if id[..16] != umid[..16] || id[16..24] != [4, 3, 2, 1, 6, 5, 8, 7] {
        return fail("the file mob ID read from the header is wrong");
    }
    Ok("file mob ID read from an MXF header".into())
}

/// Rename for Media Composer, wait for the database, rename back with a new file – all in the test folder,
/// with a journal of its own (the user's renamed.json is never touched).
fn rename_cycle(base: &Path) -> Result<String, Failed> {
    let mxf = base.join("rename/Avid MediaFiles/MXF");
    let folder = mxf.join("Avid10.1");
    io(fs::create_dir_all(&folder))?;
    io(fs::create_dir_all(mxf.join("1")))?;
    io(fs::write(folder.join("a.mxf"), b"aa"))?;
    io(fs::write(folder.join(PMR), b"old"))?;
    let jp = base.join("rename/journal.json");
    let mut j = rename::Journal::default();
    let name = rename::plan_names(std::slice::from_ref(&folder), "MFCSelftest")[0].clone();
    if rename::rename_for_avid(&mxf.join("1"), "Avid10.1", &mut j, &jp).is_ok() {
        return fail("renaming over an existing folder was allowed");
    }
    io(rename::rename_for_avid(&folder, &name, &mut j, &jp))?;
    let r = j.entries.last().cloned().ok_or_else(|| Failed::Fail("journal is empty after renaming".into()))?;
    if rename::status(&r) != rename::Status::Waiting {
        return fail("status should be “waiting for Media Composer”");
    }
    std::thread::sleep(Duration::from_millis(20));
    io(fs::write(mxf.join(&name).join(PMR), b"new"))?;
    io(fs::write(mxf.join(&name).join(MDB), b"db"))?;
    io(fs::write(mxf.join(&name).join("new.mxf"), b"n"))?;
    if rename::status(&r) != rename::Status::Ready {
        return fail("status should be “ready” after Media Composer wrote the database");
    }
    let d = io(rename::diff(&r))?;
    io(rename::rename_back(&r, &d.added, rename::Added::FirstFolder, "MFCSelftest"))?;
    if !folder.join("a.mxf").exists() || !mxf.join("1/new.mxf").exists() {
        return fail("after renaming back the files are not where they belong");
    }
    Ok(format!("renamed to “{name}”, back again, new file moved to “1”"))
}

/// Every drive with "Avid MediaFiles" answers a listing within 30 s (a hanging drive or an unanswered macOS
/// privacy prompt blocks reading). Only lists, never reads files.
fn drives(_: &Path) -> Result<String, Failed> {
    let volumes = find_volumes();
    if volumes.is_empty() {
        return Err(Failed::Skip("no drive with “Avid MediaFiles” connected".into()));
    }
    let mut slow = vec![];
    for v in &volumes {
        let dir = mxf_dir(v);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fs::read_dir(&dir).map(|d| d.count()));
        });
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => slow.push(format!("{}: {e}", report::anonymize(&v.display().to_string()))),
            Err(_) => slow.push(format!("{}: no answer within 30 s", report::anonymize(&v.display().to_string()))),
        }
    }
    if slow.is_empty() { Ok(format!("{} drive(s) readable", volumes.len())) } else { fail(slow.join("; ")) }
}

fn log_file(_: &Path) -> Result<String, Failed> {
    if cfg!(test) {
        return Err(Failed::Skip("not in unit tests (they never write the log)".into()));
    }
    let before = fs::metadata(crate::log::path()).map_or(0, |m| m.len());
    crate::log::write("self-test: log check");
    let after = fs::metadata(crate::log::path()).map_or(0, |m| m.len());
    if after > before || after > 0 && before > 900_000 { Ok("log file is written".into()) } else { fail("the log file could not be written") }
}

/// The newest release on GitHub downloads, its signature is valid and the app in it is intact.
fn update(_: &Path) -> Result<String, Failed> {
    let release = match crate::update::check(&crate::UPDATE_APP, "0.0.1") {
        Ok(Some(r)) => r,
        _ => return Err(Failed::Skip("GitHub not reachable, or no signed release yet".into())),
    };
    match crate::update::download(&release, &crate::UPDATE_APP) {
        Ok(app) => {
            let _ = fs::remove_dir_all(app.parent().unwrap_or(&app));
            Ok(format!("{}: signature valid, app intact", release.tag))
        }
        Err(crate::update::Error::Download) => Err(Failed::Skip("download failed (network)".into())),
        Err(e) => fail(format!("{}: {e:?}", release.tag)),
    }
}

fn media_composer(_: &Path) -> Result<String, Failed> {
    let running = std::process::Command::new("/usr/bin/pgrep").args(["-x", "AvidMediaComposer"]).output().is_ok_and(|o| o.status.success());
    Ok(if running { "running – renaming waits until it is closed".into() } else { "not running".into() })
}

/// Title of a check in the app's language (the report keeps the English title).
pub fn title(id: &str, lang: &str) -> String {
    let t: [&str; 4] = match id {
        "database" => ["Read Media Composer databases", "Media-Composer-Datenbanken lesen", "Lire les bases de données de Media Composer", "Leer las bases de datos de Media Composer"],
        "compare" => ["Compare a folder with its database", "Ordner mit seiner Datenbank vergleichen", "Comparer un dossier à sa base de données", "Comparar una carpeta con su base de datos"],
        "mxf" => ["Read MXF file headers", "MXF-Dateiköpfe lesen", "Lire les en-têtes MXF", "Leer las cabeceras MXF"],
        "rename" => ["Rename for Media Composer and back", "Für Media Composer umbenennen und zurück", "Renommer pour Media Composer et revenir", "Renombrar para Media Composer y volver"],
        "drives" => ["Drives with Avid MediaFiles readable", "Laufwerke mit Avid MediaFiles lesbar", "Disques avec Avid MediaFiles lisibles", "Discos con Avid MediaFiles legibles"],
        "log" => ["Log file", "Logdatei", "Fichier journal", "Archivo de registro"],
        "update" => ["Update download and signature", "Update-Download und Signatur", "Téléchargement et signature de la mise à jour", "Descarga y firma de la actualización"],
        "mediacomposer" => ["Media Composer", "Media Composer", "Media Composer", "Media Composer"],
        _ => return id.into(),
    };
    crate::pick(lang, t).into()
}

type Check = (&'static str, &'static str, fn(&Path) -> Result<String, Failed>);

/// The checks: id (stays English in the report), title (shown translated), function.
pub const CHECKS: [Check; 8] = [
    ("database", "Read Media Composer databases", database),
    ("compare", "Compare a folder with its database", compare),
    ("mxf", "Read MXF file headers", mxf_header),
    ("rename", "Rename for Media Composer and back", rename_cycle),
    ("drives", "Drives with Avid MediaFiles readable", drives),
    ("log", "Log file", log_file),
    ("update", "Update download and signature", update),
    ("mediacomposer", "Media Composer", media_composer),
];

/// Runs every check (call it off the UI thread); `progress(done, title of the next check)` after each step.
pub fn run(lang: &str, progress: &mut dyn FnMut(&[CheckResult], &str)) -> Report {
    let started = Instant::now();
    crate::log::write("self-test started");
    let base = std::env::temp_dir().join(format!("MFCSelftest-{}-{}", std::process::id(), chrono::Local::now().format("%H%M%S")));
    let mut results = vec![];
    let ready = fs::create_dir_all(&base);
    for (id, title, check) in CHECKS {
        progress(&results, title);
        let t = Instant::now();
        let outcome = match &ready {
            Ok(()) => std::panic::catch_unwind(|| check(&base)).unwrap_or_else(|_| fail("the check crashed")),
            Err(e) => fail(format!("test folder could not be created: {e}")),
        };
        let (status, detail) = match outcome {
            Ok(d) => ("pass", d),
            Err(Failed::Skip(d)) => ("skip", d),
            Err(Failed::Fail(d)) => ("fail", d),
        };
        results.push(CheckResult { id: id.into(), title: title.into(), status, detail: report::anonymize(&detail), seconds: (t.elapsed().as_secs_f64() * 100.0).round() / 100.0 });
    }
    let _ = fs::remove_dir_all(&base);
    progress(&results, "");
    let failed = results.iter().filter(|r| r.status == "fail").count();
    crate::log::write(&format!("self-test done: {} checks, {failed} failed", results.len()));
    let mut r = report::new("selftest", report::system_info(lang), report::log_tail(150));
    r.results = results;
    r.duration_seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    r
}

/// The test folder of a run, so tests can check that nothing is left behind.
pub fn leftovers() -> Vec<PathBuf> {
    fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("MFCSelftest-{}-", std::process::id()))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All checks that need no drive and no network pass, and the test folder is removed.
    #[test]
    fn checks_pass_on_their_own_data() {
        let mut steps = 0;
        let r = run("en", &mut |_, _| steps += 1);
        assert_eq!(steps, CHECKS.len() + 1);
        for c in &r.results {
            assert_ne!(c.status, "fail", "{}: {}", c.id, c.detail);
        }
        for id in ["database", "compare", "mxf", "rename"] {
            assert_eq!(r.results.iter().find(|c| c.id == id).map(|c| c.status), Some("pass"), "{id}");
        }
        assert!(leftovers().is_empty());
    }
}
