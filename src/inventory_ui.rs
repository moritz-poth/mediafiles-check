//! The "Media" tab and the media table window: scan the switched-on drives (inventory), sort the files into media,
//! renders and unknown, used or not by the switched-on projects.
use super::*;
use mediafiles_check::inventory::{self, From, Item, Kind};
use mediafiles_check::media_moves::{self, Action, Step};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct InvState {
    items: Vec<Item>,
    /// projects whose bins were read for the last scan (0: used/unused unknown)
    projects: usize,
    filter: i32,
    search: String,
    /// column, descending (default: clip name, A to Z)
    sort: (i32, bool),
    /// selected files (by path, so a new filter or sort keeps them)
    selected: HashSet<PathBuf>,
    /// items in the table, in its order
    shown: Vec<usize>,
    pub win: Win<MediaTableWindow>,
    stop: Arc<AtomicBool>,
    /// filled by the scan thread, taken by Inventory.scanned on the event loop
    result: Arc<Mutex<Option<(Vec<Item>, usize, usize)>>>,
    /// the action being confirmed and its steps; the run just done (for "Start")
    action: Option<Action>,
    plan: Vec<Step>,
    done: Option<media_moves::Run>,
}

fn journal_path() -> PathBuf {
    app_dir().unwrap_or_else(|| PathBuf::from(".")).join("media-runs.json")
}

fn lang_of(app: &AppWindow) -> String {
    app.get_language().to_string()
}

/// The History of the Media tab (also read by Tasks for databases still to rewrite).
pub fn show_runs(app: &AppWindow) {
    let lang = lang_of(app);
    let runs: media_moves::Journal = mediafiles_check::duplicates::load(&journal_path()).unwrap_or_default();
    let rows: Vec<RunRow> = runs
        .entries
        .iter()
        .map(|r| {
            let what = match r.action {
                Action::Aside => ["{n} files set aside ({size}) · {date}", "{n} Dateien beiseitegelegt ({size}) · {date}", "{n} fichiers mis de côté ({size}) · {date}", "{n} archivos apartados ({size}) · {date}"],
                Action::Renderfiles => ["{n} render files into folders of their own ({size}) · {date}", "{n} Render-Dateien in eigene Ordner ({size}) · {date}", "{n} fichiers de rendu dans leurs propres dossiers ({size}) · {date}", "{n} archivos de render en sus propias carpetas ({size}) · {date}"],
                Action::Swap => ["{n} files moved between the import and the render drive ({size}) · {date}", "{n} Dateien zwischen Import- und Render-Laufwerk verschoben ({size}) · {date}", "{n} fichiers déplacés entre le disque d’import et le disque de rendu ({size}) · {date}", "{n} archivos movidos entre la unidad de importación y la de render ({size}) · {date}"],
            };
            let text = pick(&lang, what).replace("{n}", &r.moved.len().to_string()).replace("{size}", &size_text(r.bytes())).replace("{date}", &r.date);
            RunRow { text: text.into(), db_done: r.db_done }
        })
        .collect();
    app.global::<Inventory>().set_runs(ModelRc::new(VecModel::from(rows)));
}

/// "drive / folder" of a target folder; everything below "MediaFiles Check Aside" is shown as that folder.
fn folder_label(dir: &Path) -> String {
    if dir.components().any(|c| c.as_os_str() == media_moves::ASIDE_DIR) {
        return media_moves::ASIDE_DIR.to_string();
    }
    let name = |p: Option<&Path>| p.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{} / {}", name(dir.parent().and_then(Path::parent).and_then(Path::parent)), name(Some(dir)))
}

/// One line per (source folder, target folder) with the number of files.
fn plan_text(steps: &[Step]) -> String {
    let mut lines: Vec<(String, usize)> = vec![];
    for s in steps {
        let line = format!("{} → {}", s.from.parent().map(folder_label).unwrap_or_default(), s.to.parent().map(folder_label).unwrap_or_default());
        match lines.iter_mut().find(|l| l.0 == line) {
            Some(l) => l.1 += 1,
            None => lines.push((line, 1)),
        }
    }
    lines.iter().map(|(l, n)| format!("{l}  ({n})")).collect::<Vec<_>>().join("\n")
}

/// Drives the last scan read, in order (for the two-drive choice).
fn scanned_drives(st: &InvState) -> Vec<PathBuf> {
    let mut d: Vec<PathBuf> = st.items.iter().filter_map(|i| i.path.ancestors().nth(4).map(Path::to_path_buf)).collect();
    d.sort();
    d.dedup();
    d
}

fn show_plan(app: &AppWindow, st: &InvState) {
    let g = app.global::<Inventory>();
    g.set_plan_count(st.plan.len() as i32);
    g.set_plan_size(size_text(st.plan.iter().map(|s| s.size).sum()).into());
    g.set_plan_text(plan_text(&st.plan).into());
}

/// Plans an action (0 unused aside, 1 renderfiles, 2 two drives, 3 the table's selection aside) and shows it.
fn prepare(app: &AppWindow, st: &mut InvState, kind: i32, limit: usize) {
    let g = app.global::<Inventory>();
    g.set_mc_running(media_composer_running());
    let all: Vec<&Item> = st.items.iter().collect();
    let (action, plan) = match kind {
        // unknown files are never moved by a rule – only when picked in the table
        0 => (Action::Aside, media_moves::plan_aside(&all.iter().copied().filter(|i| i.used == Some(false) && i.kind != Kind::Unknown).collect::<Vec<_>>())),
        1 => (Action::Renderfiles, media_moves::plan_renderfiles(&all, rebalance::per_folder(limit))),
        2 => {
            let drives = scanned_drives(st);
            g.set_drive_names(ModelRc::new(VecModel::from(drives.iter().map(|d| SharedString::from(volume_name(d))).collect::<Vec<_>>())));
            let (a, b) = (g.get_imports().max(0) as usize, g.get_renders().max(0) as usize);
            let (a, b) = if a < drives.len() && b < drives.len() { (a, b) } else { (0, 1.min(drives.len().saturating_sub(1))) };
            g.set_imports(a as i32);
            g.set_renders(b as i32);
            let plan = if a == b { vec![] } else { media_moves::plan_swap(&all, &drives[a], &drives[b]) };
            g.set_to_renders(plan.iter().filter(|s| s.from.starts_with(&drives[a])).count() as i32);
            g.set_to_imports(plan.iter().filter(|s| s.from.starts_with(&drives[b])).count() as i32);
            (Action::Swap, plan)
        }
        _ => (Action::Aside, media_moves::plan_aside(&all.iter().copied().filter(|i| st.selected.contains(&i.path)).collect::<Vec<_>>())),
    };
    st.action = Some(action);
    st.plan = plan;
    g.set_per_folder(rebalance::per_folder(limit) as i32);
    // asked anew for every action, kept while the drives are changed
    if g.get_task() != kind {
        g.set_delete_originals(false);
    }
    show_plan(app, st);
    g.set_task(kind);
    g.set_view(1);
    app.set_page(Page::Media);
}

fn confirm(app: &AppWindow, inv: &Inv) {
    if media_composer_running() {
        app.global::<Inventory>().set_mc_running(true);
        return;
    }
    let (action, steps) = {
        let st = inv.borrow();
        let Some(a) = st.action else { return };
        (a, st.plan.clone())
    };
    let delete = app.global::<Inventory>().get_delete_originals() && action == Action::Swap;
    log::write(&format!("media: {action:?} {} files, delete originals: {delete}", steps.len()));
    let g = app.global::<Inventory>();
    g.set_work_value(0.0);
    g.set_work_text("".into());
    g.set_task(4);
    // the table belongs to the scan before the move
    if let Some(w) = inv.borrow().win.borrow().as_ref() {
        let _ = w.hide();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let progress_to = weak.clone();
        let r = media_moves::execute(action, &steps, delete, &journal_path(), &mut |i, n, name| {
            let (w, name) = (progress_to.clone(), name.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w.upgrade() {
                    let g = w.global::<Inventory>();
                    g.set_work_value(i as f32 / n.max(1) as f32);
                    g.set_work_text(name.into());
                }
            });
        });
        let _ = tx.send(r);
    });
    let (timer, weak, inv) = (Rc::new(slint::Timer::default()), app.as_weak(), inv.clone());
    let t = timer.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
        let Ok(result) = rx.try_recv() else { return };
        t.stop();
        let Some(app) = weak.upgrade() else { return };
        let lang = lang_of(&app);
        let g = app.global::<Inventory>();
        show_runs(&app);
        // files moved: the inventory is out of date
        g.set_step(0);
        let mut st = inv.borrow_mut();
        st.items.clear();
        st.selected.clear();
        match result {
            Err(e) => {
                log::write(&format!("media: {action:?} failed: {e}"));
                g.set_task(-1);
                app.set_notice(ModelRc::new(VecModel::from(vec![notice(3, "media", &e.to_string(), 0)])));
                app.set_page(Page::Notice);
            }
            Ok((run, m)) => {
                log::write(&format!("media: {} moved, {} skipped", m.moved, m.skipped.len()));
                for (f, why) in &m.skipped {
                    log::write(&format!("  skipped {f}: {why}"));
                }
                let mut lines = vec![pick(&lang, ["{n} files moved ({size}).", "{n} Dateien verschoben ({size}).", "{n} fichiers déplacés ({size}).", "{n} archivos movidos ({size})."]).replace("{n}", &m.moved.to_string()).replace("{size}", &size_text(run.bytes()))];
                if !m.skipped.is_empty() {
                    lines.push(pick(&lang, ["Left where they are:", "Liegen geblieben:", "Laissés en place :", "Se quedaron:"]).to_string());
                    lines.extend(m.skipped.iter().take(20).map(|(f, why)| format!("· {f} – {why}")));
                }
                lines.extend(run.changed.iter().map(|f| format!("· {}", drive_and_folder(f))));
                g.set_done_summary(lines.join("\n").into());
                g.set_mc_running(media_composer_running());
                let has_run = !run.moved.is_empty();
                st.done = has_run.then_some(run);
                g.set_task(if has_run { 5 } else { -1 });
                if !has_run {
                    app.set_notice(ModelRc::new(VecModel::from(vec![notice(7, &short_list(&m.skipped.iter().map(|s| s.0.clone()).collect::<Vec<_>>()), "", m.skipped.len())])));
                    app.set_page(Page::Notice);
                }
            }
        }
    });
}

/// "Start" after a run (-1) or "Rewrite databases" of a History entry.
fn rewrite(app: &AppWindow, inv: &Inv, renames: &Rc<RefCell<Renames>>, i: i32) {
    if media_composer_running() {
        app.global::<Inventory>().set_mc_running(true);
        return;
    }
    let path = journal_path();
    let mut rows = vec![];
    match mediafiles_check::duplicates::load::<media_moves::Journal>(&path) {
        Err(e) => rows.push(notice(3, "media-runs.json", &e.to_string(), 0)),
        Ok(mut journal) => {
            let done = inv.borrow_mut().done.take();
            let index = usize::try_from(i).ok().or_else(|| done.and_then(|d| journal.entries.iter().position(|r| *r == d)));
            if let Some(run) = index.and_then(|i| journal.entries.get_mut(i)) {
                rows = rewrite_folders(&mut renames.borrow_mut(), &run.changed);
                run.db_done = true;
                if let Err(e) = mediafiles_check::duplicates::save(&journal, &path) {
                    rows.push(notice(3, &path.display().to_string(), &e.to_string(), 0));
                }
            }
        }
    }
    show_renamed(app, &mut renames.borrow_mut());
    show_runs(app);
    app.global::<Inventory>().set_task(-1);
    app.set_notice(ModelRc::new(VecModel::from(rows)));
    app.set_page(Page::Notice);
}

fn undo(app: &AppWindow, renames: &Rc<RefCell<Renames>>, i: i32) {
    if media_composer_running() {
        app.global::<Inventory>().set_mc_running(true);
        return;
    }
    let lang = lang_of(app);
    let path = journal_path();
    let mut rows = vec![];
    match mediafiles_check::duplicates::load::<media_moves::Journal>(&path) {
        Err(e) => rows.push(notice(3, "media-runs.json", &e.to_string(), 0)),
        Ok(mut journal) => {
            let Some(run) = journal.entries.get(i as usize).cloned() else { return };
            match media_moves::undo(&run) {
                Err(e) => rows.push(notice(3, "media", &e.to_string(), 0)),
                Ok(u) => {
                    log::write(&format!("media: undo, {} back, {} skipped", u.moved, u.skipped.len()));
                    rows.push(notice(5, pick(&lang, ["their folders", "ihre Ordner", "leurs dossiers", "sus carpetas"]), "", u.moved));
                    if !u.skipped.is_empty() {
                        rows.push(notice(7, &short_list(&u.skipped.iter().map(|s| s.0.clone()).collect::<Vec<_>>()), "", u.skipped.len()));
                    }
                    journal.entries.remove(i as usize);
                    if let Err(e) = mediafiles_check::duplicates::save(&journal, &path) {
                        rows.push(notice(3, &path.display().to_string(), &e.to_string(), 0));
                    }
                    // databases rewritten after the run no longer fit: rewritten again; otherwise the old ones fit again
                    if run.db_done {
                        let changed: Vec<PathBuf> = run.changed.iter().filter(|f| f.is_dir()).cloned().collect();
                        rows.extend(rewrite_folders(&mut renames.borrow_mut(), &changed));
                    }
                }
            }
        }
    }
    show_renamed(app, &mut renames.borrow_mut());
    show_runs(app);
    app.global::<Inventory>().set_step(0);
    app.set_notice(ModelRc::new(VecModel::from(rows)));
    app.set_page(Page::Notice);
}

pub type Inv = Rc<RefCell<InvState>>;

fn cache_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("media-cache.json"))
}

/// The drive switches (shared with Check and Exchange).
pub fn show_drives(app: &AppWindow, drives: &[(PathBuf, bool)]) {
    let rows: Vec<DriveRow> = drives.iter().map(|(p, sel)| DriveRow { name: volume_name(p).into(), selected: *sel }).collect();
    let g = app.global::<Inventory>();
    g.set_drive_count(drives.iter().filter(|d| d.1).count() as i32);
    g.set_drives(ModelRc::new(VecModel::from(rows)));
}

fn rate_text(r: (u32, u32)) -> String {
    let f = r.0 as f64 / r.1 as f64;
    let s = if r.1 == 1 { r.0.to_string() } else if ((f * 100.0).round() - f * 100.0).abs() < 1e-6 { format!("{f:.2}") } else { format!("{f:.3}") };
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// Length in seconds, if the header knows it.
fn seconds(i: &Item) -> Option<f64> {
    let info = i.info.as_ref()?;
    let (d, r) = (info.duration?, info.rate?);
    Some(d as f64 * r.1 as f64 / r.0 as f64)
}

/// Picture: timecode-like hh:mm:ss:ff; sound: hh:mm:ss.
fn duration_text(i: &Item) -> String {
    let (Some(info), Some(secs)) = (i.info.as_ref(), seconds(i)) else { return String::new() };
    let hms = |s: u64| format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
    match (info.picture, info.rate, info.duration) {
        (Some(_), Some(r), Some(d)) => {
            let fps = (r.0 as f64 / r.1 as f64).round().max(1.0) as u64;
            format!("{}:{:02}", hms(d / fps), d % fps)
        }
        _ => hms(secs.round() as u64),
    }
}

fn sound_text(i: &Item) -> String {
    let Some((hz, bits, ch)) = i.info.as_ref().and_then(|i| i.sound) else { return String::new() };
    let mut parts = vec![];
    if hz > 0 {
        parts.push(format!("{} kHz", rate_text((hz, 1000))));
    }
    if bits > 0 {
        parts.push(format!("{bits} bit"));
    }
    if ch > 1 {
        parts.push(format!("{ch} ch"));
    }
    parts.join(" · ")
}

fn row(i: &Item, selected: bool) -> MediaRow {
    let info = i.info.clone().unwrap_or_default();
    MediaRow {
        clip: info.clip.clone().unwrap_or_default().into(),
        kind: match i.kind {
            Kind::Media => 0,
            Kind::Render => 1,
            Kind::Unknown => 2,
        },
        from: match i.kind_from {
            From::Header => 0,
            From::Project => 1,
            From::Nothing => 2,
        },
        used: i.used.map_or(-1, |u| u as i32),
        codec: info.codec.clone().unwrap_or_default().into(),
        picture: info.picture.map(|(w, h)| format!("{w}×{h}")).unwrap_or_default().into(),
        fps: if info.picture.is_some() { info.rate.map(rate_text).unwrap_or_default() } else { String::new() }.into(),
        duration: duration_text(i).into(),
        sound: sound_text(i).into(),
        size: size_text(i.size).into(),
        project: i.project.clone().unwrap_or_default().into(),
        place: format!("{} / {}", i.drive, i.folder).into(),
        file: i.name().into(),
        selected,
    }
}

fn matches(i: &Item, filter: i32, search: &str) -> bool {
    let kind_ok = match filter {
        1 => i.kind == Kind::Media,
        2 => i.kind == Kind::Render,
        3 => i.used == Some(false),
        4 => i.kind == Kind::Unknown,
        _ => true,
    };
    if !kind_ok {
        return false;
    }
    if search.is_empty() {
        return true;
    }
    let info = i.info.clone().unwrap_or_default();
    [info.clip.unwrap_or_default(), i.name(), info.codec.unwrap_or_default(), i.project.clone().unwrap_or_default(), i.drive.clone(), i.folder.clone()]
        .iter()
        .any(|t| t.to_lowercase().contains(search))
}

fn compare(a: &Item, b: &Item, column: i32) -> std::cmp::Ordering {
    let (ia, ib) = (a.info.clone().unwrap_or_default(), b.info.clone().unwrap_or_default());
    let text = |x: Option<String>| x.unwrap_or_default().to_lowercase();
    let num = |x: Option<f64>, y: Option<f64>| x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
    match column {
        0 => text(ia.clip).cmp(&text(ib.clip)),
        1 => (a.kind as u8).cmp(&(b.kind as u8)),
        2 => (a.kind_from as u8).cmp(&(b.kind_from as u8)),
        3 => a.used.cmp(&b.used),
        4 => text(ia.codec).cmp(&text(ib.codec)),
        5 => ia.picture.map(|p| p.0 as u64 * p.1 as u64).cmp(&ib.picture.map(|p| p.0 as u64 * p.1 as u64)),
        6 => num(ia.rate.map(|r| r.0 as f64 / r.1 as f64), ib.rate.map(|r| r.0 as f64 / r.1 as f64)),
        7 => num(seconds(a), seconds(b)),
        8 => ia.sound.cmp(&ib.sound),
        9 => a.size.cmp(&b.size),
        10 => text(a.project.clone()).cmp(&text(b.project.clone())),
        11 => (a.drive.to_lowercase(), a.folder.to_lowercase()).cmp(&(b.drive.to_lowercase(), b.folder.to_lowercase())),
        _ => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
    }
    .then_with(|| a.path.cmp(&b.path))
}

/// Filter, search and sort anew and fill the table window.
fn refresh_table(st: &mut InvState) {
    let search = st.search.trim().to_lowercase();
    let mut shown: Vec<usize> = (0..st.items.len()).filter(|&i| matches(&st.items[i], st.filter, &search)).collect();
    let (col, down) = st.sort;
    shown.sort_by(|&x, &y| {
        let o = compare(&st.items[x], &st.items[y], col);
        if down { o.reverse() } else { o }
    });
    st.shown = shown;
    let Some(w) = st.win.borrow().as_ref().map(|w| w.as_weak()) else { return };
    let Some(w) = w.upgrade() else { return };
    let rows: Vec<MediaRow> = st.shown.iter().map(|&i| row(&st.items[i], st.selected.contains(&st.items[i].path))).collect();
    let count = |f: i32| st.items.iter().filter(|i| matches(i, f, "")).count() as i32;
    w.set_filter_counts(ModelRc::new(VecModel::from((0..5).map(count).collect::<Vec<_>>())));
    w.set_with_projects(st.projects > 0);
    w.set_filter(st.filter);
    w.set_sort_column(col);
    w.set_sort_up(!down);
    let chosen: Vec<&Item> = st.items.iter().filter(|i| st.selected.contains(&i.path)).collect();
    w.set_selected(chosen.len() as i32);
    w.set_selected_size(size_text(chosen.iter().map(|i| i.size).sum()).into());
    w.set_rows(ModelRc::new(VecModel::from(rows)));
}

/// Numbers of the overview.
fn show_overview(app: &AppWindow, st: &InvState) {
    let g = app.global::<Inventory>();
    let mut counts = [0i32; 7];
    let mut sizes = [0u64; 7];
    for i in &st.items {
        let mut add = |k: usize| {
            counts[k] += 1;
            sizes[k] += i.size;
        };
        match (i.kind, i.used) {
            (Kind::Unknown, _) => add(4),
            (Kind::Media, Some(false)) => add(2),
            (Kind::Render, Some(false)) => add(3),
            (Kind::Media, _) => add(0),
            (Kind::Render, _) => add(1),
        }
        match i.kind {
            Kind::Media => add(5),
            Kind::Render => add(6),
            Kind::Unknown => {}
        }
    }
    g.set_counts(ModelRc::new(VecModel::from(counts.to_vec())));
    g.set_sizes(ModelRc::new(VecModel::from(sizes.iter().map(|s| SharedString::from(size_text(*s))).collect::<Vec<_>>())));
    g.set_files(st.items.len() as i32);
    g.set_total(size_text(st.items.iter().map(|i| i.size).sum()).into());
    g.set_projects(st.projects as i32);
    g.set_unreadable(st.items.iter().filter(|i| i.error.is_some()).count() as i32);
}

fn start_scan(app: &AppWindow, st: &mut InvState, drives: &[(PathBuf, bool)], projects: &[(PathBuf, bool)]) {
    let chosen: Vec<PathBuf> = drives.iter().filter(|d| d.1).map(|d| d.0.clone()).collect();
    let projects: Vec<PathBuf> = projects.iter().filter(|p| p.1).map(|p| p.0.clone()).collect();
    log::write(&format!(
        "media scan: {} | projects: {}",
        chosen.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
        projects.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    ));
    let g = app.global::<Inventory>();
    g.set_error("".into());
    g.set_work_value(0.0);
    g.set_work_text(app.get_txt_reading_drives());
    g.set_step(1);
    st.stop = Arc::default();
    let (weak, stop, result) = (app.as_weak(), st.stop.clone(), st.result.clone());
    std::thread::spawn(move || {
        let progress_to = weak.clone();
        let last = Mutex::new(std::time::Instant::now());
        let progress = move |done: usize, total: usize| {
            // at most ten updates a second
            let mut l = last.lock().unwrap();
            if done < total && l.elapsed() < Duration::from_millis(100) {
                return;
            }
            *l = std::time::Instant::now();
            let w = progress_to.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w.upgrade() {
                    let g = w.global::<Inventory>();
                    g.set_work_value(done as f32 / total.max(1) as f32);
                    g.set_work_text(format!("{done} / {total}").into());
                }
            });
        };
        let mut items = inventory::scan(&chosen, cache_path().as_deref(), &progress, &stop);
        let refs = (!projects.is_empty() && !stop.load(Ordering::Relaxed)).then(|| projects::references(&projects, &mut |_, _, _| {}));
        inventory::classify(&mut items, refs.as_ref());
        let failed = refs.as_ref().map_or(0, |r| r.failed.len());
        log::write(&format!(
            "  {} files, {} unreadable, {} bins ({} unreadable){}",
            items.len(),
            items.iter().filter(|i| i.error.is_some()).count(),
            refs.as_ref().map_or(0, |r| r.bins),
            failed,
            if stop.load(Ordering::Relaxed) { ", stopped" } else { "" }
        ));
        for (p, e) in refs.iter().flat_map(|r| &r.failed) {
            log::write(&format!("  unreadable bin {}: {e}", p.display()));
        }
        let n = if refs.is_some() { projects.len() } else { 0 };
        *result.lock().unwrap() = Some((items, n, failed));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = weak.upgrade() {
                w.global::<Inventory>().invoke_scanned();
            }
        });
    });
}

fn open_table(app: &AppWindow, inv: &Inv, settings: &Rc<RefCell<Settings>>) {
    let win = inv.borrow().win.clone();
    if win.borrow().is_none() {
        *win.borrow_mut() = MediaTableWindow::new().ok();
        if let Some(m) = win.borrow().as_ref() {
            repaint_on_resize!(m);
            forward_menu(&m.global::<Shared>(), &app.as_weak());
            let st = inv.clone();
            m.on_filter_changed(move |f| {
                let mut s = st.borrow_mut();
                s.filter = f;
                refresh_table(&mut s);
            });
            let st = inv.clone();
            m.on_search(move |t| {
                let mut s = st.borrow_mut();
                s.search = t.to_string();
                refresh_table(&mut s);
            });
            let st = inv.clone();
            m.on_sort(move |c| {
                let mut s = st.borrow_mut();
                s.sort = if s.sort.0 == c { (c, !s.sort.1) } else { (c, false) };
                refresh_table(&mut s);
            });
            let st = inv.clone();
            m.on_toggle(move |i| {
                let mut s = st.borrow_mut();
                let Some(path) = s.shown.get(i as usize).map(|&k| s.items[k].path.clone()) else { return };
                if !s.selected.remove(&path) {
                    s.selected.insert(path);
                }
                refresh_table(&mut s);
            });
            let st = inv.clone();
            m.on_select_all(move || {
                let mut s = st.borrow_mut();
                let paths: Vec<PathBuf> = s.shown.iter().map(|&k| s.items[k].path.clone()).collect();
                s.selected.extend(paths);
                refresh_table(&mut s);
            });
            let st = inv.clone();
            m.on_select_none(move || {
                let mut s = st.borrow_mut();
                s.selected.clear();
                refresh_table(&mut s);
            });
            let (st, ww) = (inv.clone(), app.as_weak());
            m.on_save_list(move || save_list(&ww.unwrap(), &st.borrow()));
            let ww = app.as_weak();
            m.on_set_aside(move || {
                let app = ww.unwrap();
                app.global::<Inventory>().invoke_prepare(3);
                let _ = app.show();
            });
            let mw = m.as_weak();
            m.on_close(move || {
                let _ = mw.unwrap().hide();
            });
        }
    }
    refresh_table(&mut inv.borrow_mut());
    if let Some(m) = win.borrow().as_ref() {
        apply_menus(&m.global::<Shared>(), &settings.borrow());
        let _ = m.show();
    }
}

/// The shown rows as a tab-separated text file (opens in a spreadsheet).
fn save_list(app: &AppWindow, st: &InvState) {
    let lang = LANGUAGES.get(app.global::<Shared>().get_language_index() as usize).map_or("en", |l| l.0);
    let name = format!("MediaFiles Check Media {}.txt", chrono::Local::now().format("%Y-%m-%d"));
    let Some(target) = rfd::FileDialog::new().set_file_name(name).add_filter("Text", &["txt"]).save_file() else { return };
    let head = [
        ["Clip", "Clip", "Clip", "Clip"],
        ["Kind", "Art", "Type", "Tipo"],
        ["Known by", "Erkannt durch", "Reconnu par", "Reconocido por"],
        ["Used", "Benutzt", "Utilisé", "Usado"],
        ["Codec", "Codec", "Codec", "Códec"],
        ["Picture", "Bild", "Image", "Imagen"],
        ["fps", "fps", "fps", "fps"],
        ["Duration", "Dauer", "Durée", "Duración"],
        ["Sound", "Ton", "Son", "Sonido"],
        ["Size", "Größe", "Taille", "Tamaño"],
        ["Project", "Projekt", "Projet", "Proyecto"],
        ["Drive / folder", "Laufwerk / Ordner", "Disque / dossier", "Unidad / carpeta"],
        ["File", "Datei", "Fichier", "Archivo"],
    ];
    let mut text = head.iter().map(|h| pick(lang, *h)).collect::<Vec<_>>().join("\t") + "\n";
    for &k in &st.shown {
        let r = row(&st.items[k], false);
        let kind = pick(lang, [["Media", "Medium", "Média", "Medio"], ["Render", "Render", "Rendu", "Render"], ["Unknown", "Unbekannt", "Inconnu", "Desconocido"]][r.kind as usize]);
        let from = [pick(lang, ["header", "Dateikopf", "en-tête", "cabecera"]), pick(lang, ["project", "Projekt", "projet", "proyecto"]), "–"][r.from as usize];
        let used = match r.used {
            1 => pick(lang, ["yes", "ja", "oui", "sí"]),
            0 => pick(lang, ["no", "nein", "non", "no"]),
            _ => "–",
        };
        let cells = [r.clip.to_string(), kind.into(), from.into(), used.into(), r.codec.into(), r.picture.into(), r.fps.into(), r.duration.into(), r.sound.into(), r.size.into(), r.project.into(), r.place.into(), r.file.into()];
        text += &(cells.iter().map(|c| c.replace(['\t', '\n'], " ")).collect::<Vec<_>>().join("\t") + "\n");
    }
    if let Err(e) = fs::write(&target, text) {
        log::write(&format!("save media list {}: {e}", target.display()));
        app.global::<Inventory>().set_error(format!("{}: {e}", target.display()).into());
    }
}

pub fn wire(app: &AppWindow, inv: &Inv, drives: &Drives, projects: &Rc<RefCell<ProjectsState>>, settings: &Rc<RefCell<Settings>>, renames: &Rc<RefCell<Renames>>) {
    let g = app.global::<Inventory>();
    show_drives(app, &drives.borrow());
    show_runs(app);
    g.on_prepare({
        let (ww, inv, settings) = (app.as_weak(), inv.clone(), settings.clone());
        move |k| prepare(&ww.unwrap(), &mut inv.borrow_mut(), k, settings.borrow().limit)
    });
    g.on_swap_changed({
        let (ww, inv, settings) = (app.as_weak(), inv.clone(), settings.clone());
        move || prepare(&ww.unwrap(), &mut inv.borrow_mut(), 2, settings.borrow().limit)
    });
    g.on_confirm({
        let (ww, inv) = (app.as_weak(), inv.clone());
        move || confirm(&ww.unwrap(), &inv)
    });
    g.on_db({
        let (ww, inv, renames) = (app.as_weak(), inv.clone(), renames.clone());
        move |i| rewrite(&ww.unwrap(), &inv, &renames, i)
    });
    g.on_undo({
        let (ww, renames) = (app.as_weak(), renames.clone());
        move |i| undo(&ww.unwrap(), &renames, i)
    });
    g.on_toggle_drive({
        let (ww, drives) = (app.as_weak(), drives.clone());
        move |i| toggle_drive(&ww.unwrap(), &drives, i)
    });
    g.on_scan({
        let (ww, inv, drives, projects) = (app.as_weak(), inv.clone(), drives.clone(), projects.clone());
        move || start_scan(&ww.unwrap(), &mut inv.borrow_mut(), &drives.borrow(), &projects.borrow().projects)
    });
    g.on_stop({
        let inv = inv.clone();
        move || inv.borrow().stop.store(true, Ordering::Relaxed)
    });
    g.on_scanned({
        let (ww, inv) = (app.as_weak(), inv.clone());
        move || {
            let app = ww.unwrap();
            let mut st = inv.borrow_mut();
            let Some((items, projects, failed)) = st.result.lock().unwrap().take() else { return };
            let present: HashSet<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
            st.selected.retain(|p| present.contains(p));
            st.items = items;
            st.projects = projects;
            let names: Vec<SharedString> = scanned_drives(&st).iter().map(|d| SharedString::from(volume_name(d))).collect();
            app.global::<Inventory>().set_drive_names(ModelRc::new(VecModel::from(names)));
            show_overview(&app, &st);
            let g = app.global::<Inventory>();
            g.set_failed_bins(failed as i32);
            g.set_step(2);
            g.set_view(1);
            refresh_table(&mut st);
        }
    });
    g.on_show_table({
        let (ww, inv, settings) = (app.as_weak(), inv.clone(), settings.clone());
        move || open_table(&ww.unwrap(), &inv, &settings)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_and_lengths() {
        assert_eq!(rate_text((25, 1)), "25");
        assert_eq!(rate_text((30000, 1001)), "29.97");
        assert_eq!(rate_text((24000, 1001)), "23.976");
        assert_eq!(rate_text((48000, 1000)), "48");
        let mut it = Item {
            path: "/V/a.mxf".into(),
            drive: "V".into(),
            folder: "1".into(),
            size: 0,
            modified: 0,
            info: Some(mediafiles_check::mxf::Info { picture: Some((1920, 1080)), rate: Some((25, 1)), duration: Some(25 * 61 + 3), ..Default::default() }),
            error: None,
            file_id: None,
            master_id: None,
            project: None,
            in_db: false,
            kind: Kind::Media,
            kind_from: From::Header,
            used: None,
        };
        assert_eq!(duration_text(&it), "00:01:01:03");
        it.info = Some(mediafiles_check::mxf::Info { rate: Some((48000, 1)), duration: Some(48000 * 10), sound: Some((48000, 24, 2)), ..Default::default() });
        assert_eq!((duration_text(&it), sound_text(&it)), ("00:00:10".to_string(), "48 kHz · 24 bit · 2 ch".to_string()));
    }
}
