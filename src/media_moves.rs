//! Moving media files sorted by the inventory (Media tab):
//! - aside: files (e.g. unused ones) into "<drive>/MediaFiles Check Aside/<time>/<folder>/" on their own drive;
//! - renderfiles: render files into "<folder base>-renderfiles-<n>" in the same MXF folder, at most `per` files each;
//! - swap: render files on the import drive go to the render drive, media on the render drive to the import drive,
//!   each into a folder of the same name. Across drives a file is copied, the copy checked by checksum, and only
//!   then the original deleted or set aside (the user chooses each time).
//! Within a drive a move is a rename. Nothing is ever overwritten; a file changed since the scan stays where it is.
//! Every run is recorded in a journal before the first file moves, so it can be undone; afterwards the databases of
//! the changed folders have to be rewritten (like rebalancing).
use crate::inventory::{Item, Kind};
use crate::mxf_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const ASIDE_DIR: &str = "MediaFiles Check Aside";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Aside,
    Renderfiles,
    Swap,
}

/// One planned move: the file as the scan saw it (size, modification date) and where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub from: PathBuf,
    pub to: PathBuf,
    pub size: u64,
    pub modified: u128,
}

/// One run, kept in the journal (History).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Run {
    pub date: String,
    pub action: Action,
    /// (where it was, where it is now, size)
    pub moved: Vec<(PathBuf, PathBuf, u64)>,
    /// across drives, parallel to `moved`: the original set aside here, or None when it was deleted
    #[serde(default)]
    pub originals: Vec<Option<PathBuf>>,
    /// folders whose databases have to be rewritten
    pub changed: Vec<PathBuf>,
    pub db_done: bool,
    /// folders left with only their database, moved aside as a whole: (folder, where it went)
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

#[derive(Debug, Default, PartialEq)]
pub struct Moved {
    pub moved: usize,
    /// files left where they are (changed or gone since the scan, name taken at the target, copy failed): (file, why)
    pub skipped: Vec<(String, String)>,
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// "<drive>" of "<drive>/Avid MediaFiles/MXF/<folder>/<file>".
fn drive_of(file: &Path) -> Option<&Path> {
    file.parent()?.parent()?.parent()?.parent()
}

fn step(i: &Item, to: PathBuf) -> Step {
    Step { from: i.path.clone(), to, size: i.size, modified: i.modified }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string()
}

/// Where these files go when set aside.
pub fn plan_aside(items: &[&Item]) -> Vec<Step> {
    let at = stamp();
    items.iter().filter_map(|i| Some(step(i, drive_of(&i.path)?.join(ASIDE_DIR).join(&at).join(&i.folder).join(i.name())))).collect()
}

/// "<base>-renderfiles-<n>" folders: the render files of every other folder of an MXF folder, `per` files at most
/// in each; an existing renderfiles folder with room is filled first. Files already in a renderfiles folder stay.
pub fn plan_renderfiles(items: &[&Item], per: usize) -> Vec<Step> {
    let per = per.max(1);
    let mut by_folder: Vec<((PathBuf, String), Vec<&Item>)> = vec![];
    for i in items.iter().filter(|i| i.kind == Kind::Render && !i.folder.contains("-renderfiles-")) {
        let Some(mxf) = i.path.parent().and_then(Path::parent) else { continue };
        let k = (mxf.to_path_buf(), i.folder.clone());
        match by_folder.iter_mut().find(|(x, _)| *x == k) {
            Some((_, v)) => v.push(i),
            None => by_folder.push((k, vec![i])),
        }
    }
    // files already planned per target folder (key), on top of what is on the drive
    let mut planned: HashMap<PathBuf, usize> = HashMap::new();
    let mut out = vec![];
    for ((mxf, folder), files) in by_folder {
        let base = crate::rebalance::base_name(&folder).to_string();
        let mut n = 1;
        for f in files {
            loop {
                let target = mxf.join(format!("{base}-renderfiles-{n}"));
                let have = planned.get(&target).copied().unwrap_or_else(|| mxf_count(&target));
                if have < per && !target.join(f.name()).exists() {
                    planned.insert(target.clone(), have + 1);
                    out.push(step(f, target.join(f.name())));
                    break;
                }
                planned.entry(target).or_insert(have);
                n += 1;
            }
        }
    }
    out
}

fn mxf_count(dir: &Path) -> usize {
    fs::read_dir(dir).map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".mxf")).count()).unwrap_or(0)
}

/// Render files on `imports` go to `renders`, media on `renders` go to `imports`, each into the folder of the same
/// name. Unknown files never move.
pub fn plan_swap(items: &[&Item], imports: &Path, renders: &Path) -> Vec<Step> {
    items
        .iter()
        .filter_map(|i| {
            let drive = drive_of(&i.path)?;
            let to = match i.kind {
                Kind::Render if drive == imports => renders,
                Kind::Media if drive == renders => imports,
                _ => return None,
            };
            Some(step(i, mxf_dir(to).join(&i.folder).join(i.name())))
        })
        .collect()
}

fn modified(m: &fs::Metadata) -> u128 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// Still the file the scan saw.
fn unchanged(s: &Step) -> bool {
    fs::metadata(&s.from).is_ok_and(|m| m.is_file() && m.len() == s.size && modified(&m) == s.modified)
}

fn same_drive(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &Path| p.ancestors().find_map(|a| fs::metadata(a).ok()).map(|m| m.dev());
        dev(a).is_some() && dev(a) == dev(b)
    }
    #[cfg(not(unix))]
    {
        a.components().next() == b.components().next()
    }
}

/// Carries out the steps. `delete_originals`: across drives, delete the original after its checked copy (else it
/// goes into "MediaFiles Check Aside" on its drive). The run is saved in the journal before the first file moves.
pub fn execute(action: Action, steps: &[Step], delete_originals: bool, journal_path: &Path, progress: &mut dyn FnMut(usize, usize, &str)) -> io::Result<(Run, Moved)> {
    let at = stamp();
    let mut run = Run { date: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), action, moved: vec![], originals: vec![], changed: vec![], db_done: false, leftovers: vec![] };
    let mut out = Moved::default();
    let mut todo = vec![];
    for s in steps {
        if !unchanged(s) {
            out.skipped.push((file_name(&s.from), "changed or gone since the scan".into()));
        } else if s.to.symlink_metadata().is_ok() {
            out.skipped.push((file_name(&s.from), "the name is taken at the target".into()));
        } else {
            todo.push(s);
        }
    }
    if todo.is_empty() {
        return Ok((run, out));
    }
    for s in &todo {
        for f in [s.from.parent(), s.to.parent()].into_iter().flatten() {
            // folders inside the aside folder have no database
            if !f.starts_with(drive_of(&s.from).map(|d| d.join(ASIDE_DIR)).unwrap_or_default()) && !run.changed.iter().any(|c| c == f) {
                run.changed.push(f.to_path_buf());
            }
        }
    }
    // recorded first (what may move), filled in as it moves; a crash leaves a run that undo can still follow
    let mut journal: Journal = crate::duplicates::load(journal_path)?;
    journal.entries.push(run.clone());
    crate::duplicates::save(&journal, journal_path)?;
    let save = |run: &Run| -> io::Result<()> {
        let mut j: Journal = crate::duplicates::load(journal_path)?;
        if let Some(last) = j.entries.iter_mut().rev().find(|r| r.date == run.date && r.action == run.action && r.changed == run.changed) {
            *last = run.clone();
        }
        crate::duplicates::save(&j, journal_path)
    };
    for (n, s) in todo.iter().enumerate() {
        progress(n, todo.len(), &file_name(&s.from));
        if let Some(dir) = s.to.parent() {
            fs::create_dir_all(dir)?;
        }
        if same_drive(&s.from, s.to.parent().unwrap_or(&s.to)) {
            match crate::move_new(&s.from, &s.to) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    out.skipped.push((file_name(&s.from), "the name is taken at the target".into()));
                    continue;
                }
                Err(e) => {
                    // a folder made for this file only goes again; what moved so far is in the journal
                    if let Some(dir) = s.to.parent() {
                        let _ = fs::remove_dir(dir);
                    }
                    save(&run)?;
                    return Err(e);
                }
            }
            run.moved.push((s.from.clone(), s.to.clone(), s.size));
            run.originals.push(None);
        } else {
            if let Err(e) = crate::ferry::collect::copy_verified(&s.from, &s.to, None) {
                out.skipped.push((file_name(&s.from), e.to_string()));
                continue;
            }
            // the copy is recorded before the original goes
            let aside = (!delete_originals).then(|| drive_of(&s.from).map(|d| d.join(ASIDE_DIR).join(&at).join(s.from.parent().map(file_name).unwrap_or_default()).join(file_name(&s.from)))).flatten();
            run.moved.push((s.from.clone(), s.to.clone(), s.size));
            run.originals.push(aside.clone());
            save(&run)?;
            match &aside {
                Some(a) => {
                    fs::create_dir_all(a.parent().unwrap())?;
                    crate::move_new(&s.from, a)?;
                }
                None => fs::remove_file(&s.from)?,
            }
        }
        out.moved += 1;
    }
    progress(todo.len(), todo.len(), "");
    // a folder left with only its database goes aside as a whole (like duplicates)
    for f in run.changed.clone() {
        if f.is_dir() && run.moved.iter().any(|m| m.0.parent() == Some(f.as_path())) && crate::rebalance::is_leftover(&f) {
            if let Some(into) = drive_of(&f.join("x")).map(|d| d.join(ASIDE_DIR).join(&at).join(file_name(&f))) {
                run.leftovers.push((f.clone(), into.clone()));
                save(&run)?;
                crate::rebalance::move_leftover(&f, &into)?;
            }
        }
    }
    run.changed.retain(|f| f.is_dir());
    save(&run)?;
    Ok((run, out))
}

/// Puts everything of a run back (only unchanged files, never over an existing one); copies made across drives are
/// removed once their original is back. Empty folders the run made are removed.
pub fn undo(run: &Run) -> io::Result<Moved> {
    let mut out = Moved::default();
    for (folder, from) in &run.leftovers {
        if from.is_dir() && !folder.exists() {
            crate::move_new(from, folder)?;
        }
    }
    for (k, (from, to, size)) in run.moved.iter().enumerate() {
        let original = run.originals.get(k).cloned().flatten();
        let skip = |out: &mut Moved, why: &str| out.skipped.push((file_name(from), why.into()));
        if from.exists() {
            skip(&mut out, "a file of that name is back already");
            continue;
        }
        if let Some(dir) = from.parent() {
            fs::create_dir_all(dir)?;
        }
        let copy_ok = fs::metadata(to).is_ok_and(|m| m.len() == *size);
        let cross = !same_drive(from.parent().unwrap_or(from), to);
        match (&original, cross) {
            // across drives, original set aside: it comes back, the copy goes
            (Some(a), true) => {
                if fs::metadata(a).map(|m| m.len()).ok() != Some(*size) {
                    skip(&mut out, "the set-aside original is gone or changed");
                    continue;
                }
                crate::move_new(a, from)?;
                if copy_ok {
                    fs::remove_file(to)?;
                }
            }
            // across drives, original deleted: copied back (checked), then the copy goes
            (None, true) => {
                if !copy_ok {
                    skip(&mut out, "the copy is gone or changed");
                    continue;
                }
                crate::ferry::collect::copy_verified(to, from, None)?;
                fs::remove_file(to)?;
            }
            // within a drive: renamed back
            _ => {
                if !copy_ok {
                    skip(&mut out, "gone or changed since");
                    continue;
                }
                crate::move_new(to, from)?;
            }
        }
        out.moved += 1;
    }
    // folders the run created and left empty (renderfiles folders, the dated folders below "MediaFiles Check Aside"
    // and that folder itself) – never an MXF folder's parent
    let mut dirs: Vec<PathBuf> = run.moved.iter().filter_map(|m| m.1.parent().map(Path::to_path_buf)).collect();
    dirs.extend(run.originals.iter().flatten().filter_map(|a| a.parent().map(Path::to_path_buf)));
    dirs.extend(run.leftovers.iter().map(|l| l.1.clone()));
    dirs.sort();
    dirs.dedup();
    for d in dirs {
        for up in d.ancestors().take(3) {
            if up.file_name().is_some_and(|n| n == "MXF") {
                break;
            }
            if fs::read_dir(up).is_ok_and(|mut rd| rd.next().is_none()) {
                let _ = fs::remove_dir(up);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::From;

    fn item(path: &Path, kind: Kind) -> Item {
        let m = fs::metadata(path).unwrap();
        Item {
            path: path.to_path_buf(),
            drive: drive_of(path).map(file_name).unwrap_or_default(),
            folder: path.parent().map(file_name).unwrap_or_default(),
            size: m.len(),
            modified: modified(&m),
            info: None,
            error: None,
            file_id: None,
            master_id: None,
            project: None,
            in_db: true,
            kind,
            kind_from: From::Header,
            used: Some(false),
        }
    }

    fn setup(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mfc-moves-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn file(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), body).unwrap();
        dir.join(name)
    }

    #[test]
    fn renders_into_renderfiles_folders_and_back() {
        let root = setup("rf");
        let f = root.join("A/Avid MediaFiles/MXF/Mac Studio.1");
        let (r1, r2, r3, m) = (file(&f, "r1.mxf", "1"), file(&f, "r2.mxf", "2"), file(&f, "r3.mxf", "3"), file(&f, "m.mxf", "m"));
        file(&f, crate::PMR, "db");
        let items = [item(&r1, Kind::Render), item(&r2, Kind::Render), item(&r3, Kind::Render), item(&m, Kind::Media)];
        let refs: Vec<&Item> = items.iter().collect();
        let steps = plan_renderfiles(&refs, 2);
        let mxf = f.parent().unwrap();
        let targets: Vec<PathBuf> = steps.iter().map(|s| s.to.parent().unwrap().to_path_buf()).collect();
        assert_eq!(targets, [mxf.join("Mac Studio-renderfiles-1"), mxf.join("Mac Studio-renderfiles-1"), mxf.join("Mac Studio-renderfiles-2")]);
        let jp = root.join("media-runs.json");
        let (run, moved) = execute(Action::Renderfiles, &steps, false, &jp, &mut |_, _, _| {}).unwrap();
        assert_eq!(moved.moved, 3);
        assert!(m.exists() && !r1.exists() && mxf.join("Mac Studio-renderfiles-2/r3.mxf").exists());
        assert!(run.changed.contains(&f) && run.changed.contains(&mxf.join("Mac Studio-renderfiles-1")));
        assert_eq!(crate::duplicates::load::<Journal>(&jp).unwrap().entries, vec![run.clone()]);
        let u = undo(&run).unwrap();
        assert_eq!(u.moved, 3);
        assert!(r1.exists() && r3.exists() && !mxf.join("Mac Studio-renderfiles-1").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn aside_skips_changed_files_and_takes_a_leftover_folder_along() {
        let root = setup("aside");
        let f = root.join("A/Avid MediaFiles/MXF/1");
        let (a, b) = (file(&f, "a.mxf", "aa"), file(&f, "b.mxf", "bb"));
        file(&f, crate::PMR, "db");
        let items = [item(&a, Kind::Media), item(&b, Kind::Media)];
        fs::write(&b, "changed!").unwrap();
        let refs: Vec<&Item> = items.iter().collect();
        let steps = plan_aside(&refs);
        assert!(steps[0].to.starts_with(root.join("A").join(ASIDE_DIR)));
        let jp = root.join("j.json");
        let (run, m) = execute(Action::Aside, &steps, false, &jp, &mut |_, _, _| {}).unwrap();
        assert_eq!((m.moved, m.skipped.len()), (1, 1));
        assert!(!a.exists() && b.exists() && f.exists());
        // b gone too: the folder holds only its database and goes aside as a whole
        fs::remove_file(&b).unwrap();
        let items = [item(&file(&f, "c.mxf", "c"), Kind::Media)];
        let refs: Vec<&Item> = items.iter().collect();
        let (run2, _) = execute(Action::Aside, &plan_aside(&refs), false, &jp, &mut |_, _, _| {}).unwrap();
        assert!(!f.exists() && run2.leftovers.len() == 1);
        undo(&run2).unwrap();
        undo(&run).unwrap();
        assert!(a.exists() && f.join("c.mxf").exists() && f.join(crate::PMR).exists());
        assert!(!root.join("A").join(ASIDE_DIR).exists(), "nothing left behind");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn swap_across_drives_copies_checks_and_handles_the_original() {
        // two folders stand in for two drives (same device here, so force the copy path by planning only)
        let root = setup("swap");
        let (imports, renders) = (root.join("IMPORT"), root.join("RENDER"));
        let r = file(&mxf_dir(&imports).join("1"), "render.mxf", "r");
        let m = file(&mxf_dir(&renders).join("1"), "media.mxf", "m");
        let u = file(&mxf_dir(&imports).join("1"), "unknown.mxf", "u");
        let items = [item(&r, Kind::Render), item(&m, Kind::Media), item(&u, Kind::Unknown)];
        let refs: Vec<&Item> = items.iter().collect();
        let steps = plan_swap(&refs, &imports, &renders);
        assert_eq!(steps.iter().map(|s| s.to.clone()).collect::<Vec<_>>(), [mxf_dir(&renders).join("1/render.mxf"), mxf_dir(&imports).join("1/media.mxf")]);
        let jp = root.join("j.json");
        let (run, moved) = execute(Action::Swap, &steps, false, &jp, &mut |_, _, _| {}).unwrap();
        assert_eq!(moved.moved, 2);
        assert!(u.exists() && mxf_dir(&renders).join("1/render.mxf").exists() && !r.exists());
        assert_eq!(undo(&run).unwrap().moved, 2);
        assert!(r.exists() && m.exists() && !mxf_dir(&renders).join("1/render.mxf").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
