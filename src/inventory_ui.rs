//! The "Media" tab and the media table window: scan the switched-on drives (inventory), sort the files into media,
//! renders and unknown, used or not by the switched-on projects.
use super::*;
use mediafiles_check::inventory::{self, From, Item, Kind};
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

pub fn wire(app: &AppWindow, inv: &Inv, drives: &Drives, projects: &Rc<RefCell<ProjectsState>>, settings: &Rc<RefCell<Settings>>) {
    let g = app.global::<Inventory>();
    show_drives(app, &drives.borrow());
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
