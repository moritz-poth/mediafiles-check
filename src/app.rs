//! App: check all drives or a chosen folder, show the differences, save them as a text report;
//! rename folders so Media Composer rebuilds their databases and rename them back afterwards.
#![cfg_attr(windows, windows_subsystem = "windows")]

use mediafiles_check::rename::{self, Added, Journal, Renamed, Status};
use mediafiles_check::projects::{self, ClipKind, ScanResult};
use mediafiles_check::{FolderReport, check, find_volumes, log, report_text};
use std::collections::{HashSet, VecDeque};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

slint::include_modules!();

/// (code, name in its own language, manual). New language: add it here, translations/<code>/LC_MESSAGES/mediafiles-check.po
/// and docs/help/<code>.md.
const LANGUAGES: [(&str, &str, &str); 4] = [
    ("en", "English", include_str!("../docs/help/en.md")),
    ("de", "Deutsch", include_str!("../docs/help/de.md")),
    ("fr", "Français", include_str!("../docs/help/fr.md")),
    ("es", "Español", include_str!("../docs/help/es.md")),
];
/// File names shown per finding; the saved report has all of them.
const NAMES_SHOWN: usize = 10;

#[derive(Clone)]
struct Settings {
    language: String,
    dark: bool,
}

fn app_dir() -> Option<PathBuf> {
    let (var, sub) = if cfg!(windows) { ("APPDATA", "MediaFiles Check") } else { ("HOME", "Library/Application Support/MediaFiles Check") };
    std::env::var_os(var).map(|b| PathBuf::from(b).join(sub))
}

fn settings_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("settings.txt"))
}

/// Original names and media files of renamed folders – needed to rename them back.
fn journal_path() -> PathBuf {
    app_dir().unwrap_or_else(|| PathBuf::from(".")).join("renamed.json")
}

/// Media Composer's executable is "AvidMediaComposer"; matched exactly, because helpers like
/// AvidBackgroundServicesManager live inside its app bundle and run all the time.
fn media_composer_running() -> bool {
    std::process::Command::new("pgrep").args(["-x", "AvidMediaComposer"]).output().is_ok_and(|o| o.status.success())
}

/// settings.txt: language on the first line, "dark" or "light" on the second.
fn load_settings() -> Settings {
    let text = settings_path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let mut lines = text.lines();
    let language = lines.next().filter(|l| LANGUAGES.iter().any(|x| x.0 == *l)).unwrap_or("en").to_string();
    Settings { language, dark: lines.next() != Some("light") }
}

fn save_settings(s: &Settings) {
    // comfort only; if saving fails the defaults apply next time
    if let Some(p) = settings_path() {
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::write(p, format!("{}\n{}\n", s.language, if s.dark { "dark" } else { "light" }));
    }
}

fn apply_menus(shared: &Shared, s: &Settings) {
    let languages: Vec<MenuChoice> = LANGUAGES.iter().map(|l| MenuChoice { title: l.1.into(), checked: l.0 == s.language }).collect();
    shared.set_languages(ModelRc::new(VecModel::from(languages)));
    shared.set_appearance(ModelRc::new(VecModel::from(vec![s.dark, !s.dark])));
    let names: Vec<slint::SharedString> = LANGUAGES.iter().map(|l| l.1.into()).collect();
    shared.set_language_names(ModelRc::new(VecModel::from(names)));
    shared.set_language_index(LANGUAGES.iter().position(|l| l.0 == s.language).unwrap_or(0) as i32);
    shared.set_dark(s.dark);
}

fn volume_name(p: &std::path::Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Connected drives with "Avid MediaFiles/MXF" and whether they are ticked.
type Drives = Rc<RefCell<Vec<(PathBuf, bool)>>>;

fn show_drives(app: &AppWindow, drives: &[(PathBuf, bool)]) {
    let rows: Vec<DriveRow> = drives.iter().map(|(p, sel)| DriveRow { name: volume_name(p).into(), selected: *sel }).collect();
    app.set_selected_count(drives.iter().filter(|d| d.1).count() as i32);
    app.set_drives(ModelRc::new(VecModel::from(rows)));
}

/// Picks up drives that were connected or ejected; ticks stay, new drives come ticked.
fn sync_drives(app: &AppWindow, drives: &Drives) {
    let found = find_volumes();
    let mut d = drives.borrow_mut();
    if found.iter().eq(d.iter().map(|x| &x.0)) {
        return;
    }
    *d = found.into_iter().map(|p| {
        let selected = d.iter().find(|x| x.0 == p).is_none_or(|x| x.1);
        (p, selected)
    }).collect();
    show_drives(app, &d);
}

/// Splits the manual into blocks: "# " = subtitle, "## " = heading, every paragraph its own block
/// (StyledText knows neither headings nor paragraph spacing). Backticks go, so everything stays in the app font.
fn help_blocks(markdown: &str) -> (String, Vec<HelpBlock>) {
    let mut subtitle = String::new();
    let mut blocks = vec![];
    let mut paragraph = String::new();
    let flush = |paragraph: &mut String, blocks: &mut Vec<HelpBlock>| {
        // "<folder>" would otherwise be an HTML tag
        let text = paragraph.trim().replace('`', "").replace('<', "‹").replace('>', "›");
        if !text.is_empty() {
            let body = slint::StyledText::from_markdown(&text).unwrap_or_else(|_| slint::StyledText::from_plain_text(&text));
            blocks.push(HelpBlock { heading: "".into(), body });
        }
        paragraph.clear();
    };
    for line in markdown.lines() {
        if let Some(t) = line.strip_prefix("# ") {
            subtitle = t.to_uppercase();
        } else if let Some(h) = line.strip_prefix("## ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(HelpBlock { heading: h.into(), body: Default::default() });
        } else if line.trim().is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            paragraph.push_str(line);
            paragraph.push('\n');
        }
    }
    flush(&mut paragraph, &mut blocks);
    (subtitle, blocks)
}

fn update_help(help: &HelpWindow, s: &Settings) {
    let markdown = LANGUAGES.iter().find(|l| l.0 == s.language).unwrap_or(&LANGUAGES[0]).2;
    let (subtitle, blocks) = help_blocks(markdown);
    help.set_subtitle(subtitle.into());
    help.set_blocks(ModelRc::new(VecModel::from(blocks)));
    apply_menus(&help.global::<Shared>(), s);
}

/// Menu of a second window (manual, About, settings) forwards to the main window.
fn forward_menu(shared: &Shared, weak: &slint::Weak<AppWindow>) {
    let w = weak.clone();
    shared.on_select_language(move |i| w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_select_language(i)));
    let w = weak.clone();
    shared.on_select_dark(move |d| w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_select_dark(d)));
    let w = weak.clone();
    shared.on_show_help(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_help()));
    let w = weak.clone();
    shared.on_show_about(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_about()));
    let w = weak.clone();
    shared.on_show_log(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_log()));
}

type Win<T> = Rc<RefCell<Option<T>>>;

/// The windows besides the main one, created when first opened.
#[derive(Clone, Default)]
struct Others {
    about: Win<AboutWindow>,
    help: Win<HelpWindow>,
    settings: Win<SettingsWindow>,
    projects: Win<ProjectsWindow>,
}

/// Language or appearance changed: every open window follows.
fn apply_all(app: &AppWindow, others: &Others, s: &Settings) {
    apply_menus(&app.global::<Shared>(), s);
    others.about.borrow().iter().for_each(|a| apply_menus(&a.global::<Shared>(), s));
    others.settings.borrow().iter().for_each(|w| apply_menus(&w.global::<Shared>(), s));
    others.projects.borrow().iter().for_each(|w| apply_menus(&w.global::<Shared>(), s));
    others.help.borrow().iter().for_each(|h| update_help(h, s));
    #[cfg(target_os = "macos")]
    {
        // Slint rebuilds the menu after this event; add the item back once that is done
        let title = app.get_txt_settings_menu().to_string();
        slint::Timer::single_shot(Duration::from_millis(150), move || mac_menu::set_settings_title(&title));
    }
}

fn rows(reports: &[FolderReport], lang: &str, more: &str) -> Vec<ReportRow> {
    let mut rows = vec![];
    for r in reports.iter().filter(|r| !r.findings.is_empty()) {
        rows.push(ReportRow { text: r.place.clone().into(), level: 0 });
        for f in &r.findings {
            rows.push(ReportRow { text: f.describe(r.files, lang).into(), level: 1 });
            let names = f.names();
            rows.extend(names.iter().take(NAMES_SHOWN).map(|n| ReportRow { text: n.clone().into(), level: 2 }));
            if names.len() > NAMES_SHOWN {
                rows.push(ReportRow { text: more.replace("{}", &(names.len() - NAMES_SHOWN).to_string()).into(), level: 2 });
            }
        }
    }
    rows
}

/// Result of the last check: reports and the folders that can be renamed for Media Composer (folder, new name).
#[derive(Default)]
struct Checked {
    reports: Vec<FolderReport>,
    candidates: Vec<(PathBuf, String)>,
}

type Last = Arc<Mutex<Checked>>;

fn show_candidates(app: &AppWindow, c: &[(PathBuf, String)]) {
    let rows: Vec<RenameRow> = c.iter().map(|(f, to)| RenameRow { from: volume_name(f).into(), to: to.clone().into() }).collect();
    app.set_rename_count(c.len() as i32);
    app.set_rename_rows(ModelRc::new(VecModel::from(rows)));
}

fn start_check(app: &AppWindow, roots: Vec<PathBuf>, last: Last) {
    app.set_error("".into());
    app.set_work_value(0.0);
    app.set_work_text("".into());
    app.set_page(Page::Working);
    let weak = app.as_weak();
    log::write(&format!("check {}", roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")));
    std::thread::spawn(move || {
        let progress = weak.clone();
        let reports = check(&roots, &mut |i, n, place| {
            let (place, w) = (place.to_string(), progress.clone());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(a) = w.upgrade() {
                    a.set_work_text(place.into());
                    a.set_work_value(i as f32 / n.max(1) as f32);
                }
            });
        });
        let bad: Vec<&FolderReport> = reports.iter().filter(|r| !r.findings.is_empty()).collect();
        log::write(&format!("checked {} folders, {} with differences", reports.len(), bad.len()));
        for r in bad {
            let findings: Vec<String> = r.findings.iter().map(|f| f.describe(r.files, "en")).collect();
            log::write(&format!("  {}: {}", r.path.display(), findings.join("; ")));
        }
        let computer = rename::computer_name();
        let folders: Vec<PathBuf> = reports.iter().filter(|r| !r.findings.is_empty() && rename::needs_rename(&r.path, &computer)).map(|r| r.path.clone()).collect();
        let candidates: Vec<(PathBuf, String)> = folders.iter().cloned().zip(rename::plan_names(&folders, &computer)).collect();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            let lang = app.get_language().to_string();
            show_candidates(&app, &candidates);
            app.set_checked_folders(reports.len() as i32);
            app.set_bad_folders(reports.iter().filter(|r| !r.findings.is_empty()).count() as i32);
            app.set_rows(ModelRc::new(VecModel::from(rows(&reports, &lang, &app.get_txt_more()))));
            *last.lock().unwrap() = Checked { reports, candidates };
            app.set_page(Page::Result);
        });
    });
}

/// A second window that changes its height does not always repaint the new area (Slint 1.18 on macOS: the frame
/// is drawn before the resize is done and nothing asks again). Ask a few times while the resize settles.
/// Every window grows with its content: redraw after each size change, or the new area stays unpainted.
macro_rules! repaint_on_resize {
    ($w:expr) => {{
        let weak = $w.as_weak();
        $w.global::<Shared>().on_resized(move || weak.upgrade().iter().for_each(repaint_soon));
    }};
}

fn repaint_soon<T: ComponentHandle + 'static>(c: &T) {
    for ms in [50, 200, 600, 1500] {
        let weak = c.as_weak();
        slint::Timer::single_shot(Duration::from_millis(ms), move || weak.upgrade().into_iter().for_each(|c| c.window().request_redraw()));
    }
}

/// Long names shortened in the middle, so start and end stay visible (monospaced font: characters = width).
fn shorten(s: &str, max: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= max {
        return s.to_string();
    }
    let head = (max - 1) / 2;
    c[..head].iter().chain(['…'].iter()).chain(c[c.len() - (max - 1 - head)..].iter()).collect()
}

/// Avid projects window: chosen folder, projects with their switch, last scan.
#[derive(Default)]
struct ProjectsState {
    projects: Vec<(PathBuf, bool)>,
    result: Arc<Mutex<Option<ScanResult>>>,
    /// opened media file groups in the result list
    open: HashSet<usize>,
}

fn projects_folder_file() -> Option<PathBuf> {
    app_dir().map(|d| d.join("projects-folder.txt"))
}

/// Projects below the folder, all switched off (the user picks what to scan).
fn load_projects(w: &ProjectsWindow, st: &mut ProjectsState, folder: &Path) {
    st.projects = projects::find_projects(folder).into_iter().map(|p| (p, false)).collect();
    w.set_folder(shorten(&folder.display().to_string(), 60).into());
    show_projects(w, st);
}

fn show_projects(w: &ProjectsWindow, st: &ProjectsState) {
    let rows: Vec<DriveRow> = st.projects.iter().map(|(p, on)| DriveRow { name: volume_name(p).into(), selected: *on }).collect();
    w.set_selected_count(st.projects.iter().filter(|p| p.1).count() as i32);
    w.set_projects(ModelRc::new(VecModel::from(rows)));
}

/// Rows of the result: per bin its missing media files; an open file shows where it is used in the bin.
/// Capped at whole bins (the saved list has all); group indexes count over all bins, so they stay stable.
fn project_rows(w: &ProjectsWindow, r: &ScanResult, open: &HashSet<usize>) -> Vec<MissingRow> {
    const MAX_ROWS: usize = 1000;
    let files = |n: usize| if n == 1 { w.get_txt_one_file() } else { w.get_txt_files_n().replace("{}", &n.to_string()).into() };
    let row = |level, text: String, note: SharedString, open, index| MissingRow { level, text: text.into(), note, open, index };
    let mut rows = vec![];
    let mut index = 0;
    for ((project, bin), groups) in &r.by_bin {
        if rows.len() >= MAX_ROWS {
            rows.push(row(2, w.get_txt_more_rows().into(), "".into(), false, -1));
            break;
        }
        rows.push(row(0, format!("{project} / {bin}"), files(groups.iter().map(|g| g.files).sum()), false, -1));
        for g in groups {
            let is_open = open.contains(&index);
            let count = if g.files > 1 { files(g.files).to_string() } else { String::new() };
            let note = match (count.is_empty(), g.relink) {
                (_, false) => count,
                (true, true) => w.get_txt_relink().to_string(),
                (false, true) => format!("{count} · {}", w.get_txt_relink()),
            };
            rows.push(row(1, g.name.clone(), note.into(), is_open, index as i32));
            if is_open {
                for (clip, kind) in &g.uses {
                    let kind = match kind {
                        ClipKind::Master => "".into(),
                        ClipKind::Subclip => w.get_txt_subclip(),
                        ClipKind::Sequence => w.get_txt_sequence(),
                    };
                    rows.push(row(2, clip.clone(), kind, false, -1));
                }
            }
            index += 1;
        }
    }
    rows
}

fn start_project_scan(w: &ProjectsWindow, st: &mut ProjectsState) {
    st.open.clear();
    let chosen: Vec<PathBuf> = st.projects.iter().filter(|p| p.1).map(|p| p.0.clone()).collect();
    log::write(&format!("project scan: {}", chosen.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
    w.set_error("".into());
    w.set_work_value(0.0);
    w.set_work_text(w.get_txt_reading_drives());
    w.set_step(1);
    let (weak, result) = (w.as_weak(), st.result.clone());
    std::thread::spawn(move || {
        let drives = find_volumes();
        let a = projects::available_media(&drives, Duration::from_secs(30));
        log::write(&format!("  {} media files on {} drives, {} in no database identified by their MXF header, {} unidentified", a.ids.len(), drives.len(), a.from_header, a.unknown));
        for d in &a.skipped {
            log::write(&format!("  drive does not answer, skipped: {}", d.display()));
        }
        let (available, unknown) = (a.ids, a.unknown);
        let skipped: Vec<String> = a.skipped.iter().map(|d| volume_name(d)).collect();
        let progress = weak.clone();
        let r = projects::scan(&chosen, &available, &mut |i, n, bin| {
            let (bin, w) = (bin.to_string(), progress.clone());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w.upgrade() {
                    w.set_work_text(bin.into());
                    w.set_work_value(i as f32 / n.max(1) as f32);
                }
            });
        });
        log::write(&format!("  {} bins ({} unreadable), {} clips, {} media files missing, {} to relink, {} AMA left out, {} renders", r.bins, r.failed.len(), r.clips, r.missing.len(), r.relink, r.ama, r.renders));
        for (p, e) in &r.failed {
            log::write(&format!("  unreadable bin {}: {e}", p.display()));
        }
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = weak.upgrade() else { return };
            w.set_missing(r.missing.len() as i32);
            w.set_unmanaged(r.missing.iter().filter(|m| m.unmanaged).count() as i32);
            w.set_bins(r.bins as i32);
            w.set_clips(r.clips as i32);
            w.set_failed(r.failed.len() as i32);
            w.set_unknown(unknown as i32);
            w.set_skipped(skipped.join(", ").into());
            w.set_renders(r.renders as i32);
            w.set_relink(r.relink as i32);
            w.set_ama(r.ama as i32);
            w.set_rows(ModelRc::new(VecModel::from(project_rows(&w, &r, &HashSet::new()))));
            *result.lock().unwrap() = Some(r);
            w.set_step(2);
        });
    });
}

fn open_projects(weak: &slint::Weak<AppWindow>, win: &Win<ProjectsWindow>, st: &Rc<RefCell<ProjectsState>>, settings: &Settings) {
    let mut win = win.borrow_mut();
    if win.is_none() {
        *win = ProjectsWindow::new().ok();
        if let Some(w) = win.as_ref() {
            repaint_on_resize!(w);
            forward_menu(&w.global::<Shared>(), weak);
            let saved = projects_folder_file().and_then(|f| fs::read_to_string(f).ok()).map(|s| PathBuf::from(s.trim()));
            if let Some(folder) = saved.filter(|f| f.is_dir()) {
                load_projects(w, &mut st.borrow_mut(), &folder);
            }
            w.on_choose_folder({
                let (ww, st) = (w.as_weak(), st.clone());
                move || {
                    let w = ww.unwrap();
                    let Some(folder) = rfd::FileDialog::new().set_title(w.get_txt_choose()).pick_folder() else { return };
                    if let Some(f) = projects_folder_file() {
                        let _ = fs::create_dir_all(f.parent().unwrap());
                        let _ = fs::write(f, folder.display().to_string());
                    }
                    load_projects(&w, &mut st.borrow_mut(), &folder);
                }
            });
            w.on_toggle({
                let (ww, st) = (w.as_weak(), st.clone());
                move |i| {
                    let mut st = st.borrow_mut();
                    if let Some(p) = st.projects.get_mut(i as usize) {
                        p.1 = !p.1;
                    }
                    show_projects(&ww.unwrap(), &st);
                }
            });
            w.on_scan({
                let (ww, st) = (w.as_weak(), st.clone());
                move || start_project_scan(&ww.unwrap(), &mut st.borrow_mut())
            });
            w.on_toggle_group({
                let (ww, st) = (w.as_weak(), st.clone());
                move |i| {
                    let w = ww.unwrap();
                    let mut st = st.borrow_mut();
                    let i = i as usize;
                    if !st.open.remove(&i) {
                        st.open.insert(i);
                    }
                    if let Some(r) = st.result.lock().unwrap().as_ref() {
                        w.set_rows(ModelRc::new(VecModel::from(project_rows(&w, r, &st.open))));
                    }
                        }
            });
            w.on_back({
                let ww = w.as_weak();
                move || {
                    let w = ww.unwrap();
                    w.set_step(0);
                        }
            });
            w.on_save_list({
                let (ww, st) = (w.as_weak(), st.clone());
                move || {
                    let w = ww.unwrap();
                    let lang = w.global::<Shared>().get_language_index();
                    let lang = LANGUAGES.get(lang as usize).map_or("en", |l| l.0);
                    let name = format!("MediaFiles Check Projects {}.txt", chrono::Local::now().format("%Y-%m-%d"));
                    let Some(target) = rfd::FileDialog::new().set_file_name(name).add_filter("Text", &["txt"]).save_file() else { return };
                    let text = st.borrow().result.lock().unwrap().as_ref().map(|r| projects::list_text(r, lang)).unwrap_or_default();
                    if let Err(e) = fs::write(&target, text) {
                        log::write(&format!("save list {}: {e}", target.display()));
                        w.set_error(format!("{}: {e}", target.display()).into());
                    }
                }
            });
            w.on_save_request({
                let (ww, st) = (w.as_weak(), st.clone());
                move || {
                    let w = ww.unwrap();
                    let system = rename::computer_name();
                    let name = format!("{}_{}_Request.ferry", chrono::Local::now().format("%y%m%d-%H%M"), system.replace(['/', '\\', ':'], "_"));
                    let Some(target) = rfd::FileDialog::new().set_file_name(name).add_filter("MediaFiles Ferry", &["ferry"]).save_file() else { return };
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let data = st.borrow().result.lock().unwrap().as_ref().map(|r| projects::request_json(&system, now, &r.missing)).unwrap_or_default();
                    let result = fs::write(&target, data);
                    log::write(&format!("request saved {}: {result:?}", target.display()));
                    if let Err(e) = result {
                        w.set_error(format!("{}: {e}", target.display()).into());
                    }
                }
            });
        }
    }
    if let Some(w) = win.as_ref() {
        apply_menus(&w.global::<Shared>(), settings);
        let _ = w.show();
    }
}

fn notice(kind: i32, a: &str, b: &str, n: usize) -> NoticeRow {
    NoticeRow { kind, a: a.into(), b: b.into(), n: n as i32 }
}

enum Question {
    /// Media Composer has not rewritten the database yet
    NotReady,
    Changed(Vec<String>),
    Added(Vec<String>),
}

struct Current {
    r: Renamed,
    questions: VecDeque<Question>,
    added: Vec<String>,
    choice: Added,
}

/// Renaming back, one folder after the other, with questions in between.
struct BackFlow {
    todo: VecDeque<Renamed>,
    cur: Option<Current>,
    notes: Vec<NoticeRow>,
}

struct Renames {
    computer: String,
    /// Err: the journal could not be read – renaming stays off so it is never overwritten
    journal: Result<Journal, String>,
    back: Option<BackFlow>,
}

impl Renames {
    fn load() -> Renames {
        Renames { computer: rename::computer_name(), journal: Journal::load(&journal_path()).map_err(|e| e.to_string()), back: None }
    }
}

fn show_renamed(app: &AppWindow, st: &Renames) {
    let entries = st.journal.as_ref().map(|j| j.entries.as_slice()).unwrap_or_default();
    let rows: Vec<RenamedRow> = entries
        .iter()
        .map(|r| {
            let status = match rename::status(r) {
                Status::Offline => 0,
                Status::Waiting => 1,
                Status::Ready => 2,
            };
            RenamedRow { current: r.current.clone().into(), original: r.original.clone().into(), status }
        })
        .collect();
    app.set_mc_running(!rows.is_empty() && media_composer_running());
    app.set_renamed(ModelRc::new(VecModel::from(rows)));
}

fn show_question(app: &AppWindow, cur: &Current, computer: &str) {
    let (kind, names): (i32, &[String]) = match cur.questions.front() {
        Some(Question::Added(n)) => (0, n),
        Some(Question::Changed(n)) => (1, n),
        _ => (2, &[]),
    };
    let mut shown: Vec<slint::SharedString> = names.iter().take(NAMES_SHOWN).map(|n| n.clone().into()).collect();
    if names.len() > NAMES_SHOWN {
        shown.push(app.get_txt_more().replace("{}", &(names.len() - NAMES_SHOWN).to_string()).into());
    }
    let first = if rename::is_network(&cur.r.dir()) { format!("{computer}.1") } else { "1".into() };
    app.set_q_kind(kind);
    app.set_q_folder(cur.r.current.clone().into());
    app.set_q_original(cur.r.original.clone().into());
    app.set_q_count(names.len() as i32);
    app.set_q_names(ModelRc::new(VecModel::from(shown)));
    app.set_q_first(first.into());
    app.set_page(Page::Question);
}

/// Next step of renaming back: ask the next question, rename the current folder back, or take the next folder.
fn advance(app: &AppWindow, st: &mut Renames) {
    let Renames { computer, journal, back } = st;
    let (Ok(journal), Some(flow)) = (journal.as_mut(), back.as_mut()) else { return };
    loop {
        if let Some(cur) = flow.cur.as_ref() {
            if !cur.questions.is_empty() {
                return show_question(app, cur, computer);
            }
            let cur = flow.cur.take().unwrap();
            let result = rename::rename_back(&cur.r, &cur.added, cur.choice, computer);
            log::write(&format!(
                "rename back {} -> {} ({} new files, {:?}): {}",
                cur.r.dir().display(),
                cur.r.original,
                cur.added.len(),
                cur.choice,
                result.as_ref().map_or_else(|e| e.to_string(), |b| format!("{b:?}"))
            ));
            match result {
                Ok(b) => {
                    flow.notes.push(notice(0, &cur.r.current, &cur.r.original, 0));
                    if let Some((to, n)) = b.moved_to.filter(|m| m.1 > 0) {
                        flow.notes.push(notice(1, &to, "", n));
                    }
                    flow.notes.extend(b.not_moved.iter().map(|f| notice(4, f, "", 0)));
                    journal.entries.retain(|e| *e != cur.r);
                    if let Err(e) = journal.save(&journal_path()) {
                        flow.notes.push(notice(3, &journal_path().display().to_string(), &e.to_string(), 0));
                    }
                }
                Err(e) => flow.notes.push(notice(3, &cur.r.current, &e.to_string(), 0)),
            }
            continue;
        }
        let Some(r) = flow.todo.pop_front() else { break };
        match rename::diff(&r) {
            Ok(d) => {
                let mut questions = VecDeque::new();
                if rename::status(&r) == Status::Waiting {
                    questions.push_back(Question::NotReady);
                }
                if !d.changed.is_empty() {
                    questions.push_back(Question::Changed(d.changed));
                }
                if !d.added.is_empty() {
                    questions.push_back(Question::Added(d.added.clone()));
                }
                flow.cur = Some(Current { r, questions, added: d.added, choice: Added::Keep });
            }
            Err(e) => flow.notes.push(notice(3, &r.current, &e.to_string(), 0)),
        }
    }
    let notes = std::mem::take(&mut flow.notes);
    *back = None;
    show_renamed(app, st);
    app.set_notice(ModelRc::new(VecModel::from(notes)));
    app.set_page(Page::Notice);
}

/// macOS app menu (Slint's winit backend builds it itself, without a Settings item):
/// "About" goes to our About window instead of the standard panel, and "Settings…" (⌘,) is added after it.
/// Slint rebuilds the menu (language change, other window in front), so the item is re-added in `setMainMenu:`.
#[cfg(target_os = "macos")]
mod mac_menu {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{class, msg_send, sel};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Handler = RefCell<Option<Box<dyn Fn()>>>;
    thread_local! {
        static ABOUT: Handler = RefCell::new(None);
        static SETTINGS: Handler = RefCell::new(None);
        static TITLE: RefCell<String> = RefCell::new("Settings…".into());
    }
    static ORIGINAL_SET_MAIN_MENU: AtomicUsize = AtomicUsize::new(0);
    type ObjFn = extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);

    extern "C-unwind" fn about(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
        ABOUT.with(|h| h.borrow().iter().for_each(|f| f()));
    }

    extern "C-unwind" fn settings(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
        SETTINGS.with(|h| h.borrow().iter().for_each(|f| f()));
    }

    extern "C-unwind" fn set_main_menu(app: *mut AnyObject, sel: Sel, menu: *mut AnyObject) {
        let original = ORIGINAL_SET_MAIN_MENU.load(Ordering::Relaxed);
        if original != 0 {
            let original: ObjFn = unsafe { std::mem::transmute(original) };
            original(app, sel, menu);
        }
        ensure();
    }

    fn ns_string(s: &str) -> Retained<AnyObject> {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    /// Adds "Settings…" to the app menu if it is missing and keeps its title in the current language.
    pub fn ensure() {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let main: *mut AnyObject = msg_send![app, mainMenu];
            if main.is_null() {
                return;
            }
            let count: isize = msg_send![main, numberOfItems];
            if count == 0 {
                return;
            }
            let first: *mut AnyObject = msg_send![main, itemAtIndex: 0isize];
            let app_menu: *mut AnyObject = msg_send![first, submenu];
            if app_menu.is_null() {
                return;
            }
            let title = TITLE.with(|t| ns_string(&t.borrow()));
            let index: isize = msg_send![app_menu, indexOfItemWithTarget: std::ptr::null_mut::<AnyObject>(), andAction: sel!(mfcShowSettings:)];
            if index >= 0 {
                let item: *mut AnyObject = msg_send![app_menu, itemAtIndex: index];
                let _: () = msg_send![item, setTitle: &*title];
                return;
            }
            // only into the app menu Slint builds (About, separator, …); macOS sets its own short menu at launch,
            // and inserting past its end throws an Objective-C exception that aborts the app
            let items: isize = msg_send![app_menu, numberOfItems];
            if items < 3 {
                return;
            }
            let second: *mut AnyObject = msg_send![app_menu, itemAtIndex: 1isize];
            let separator_at_1: bool = msg_send![second, isSeparatorItem];
            if !separator_at_1 {
                return;
            }
            let item: Retained<AnyObject> = msg_send![class!(NSMenuItem), new];
            let _: () = msg_send![&*item, setTitle: &*title];
            let _: () = msg_send![&*item, setAction: sel!(mfcShowSettings:)];
            let _: () = msg_send![&*item, setKeyEquivalent: &*ns_string(",")];
            let separator: Retained<AnyObject> = msg_send![class!(NSMenuItem), separatorItem];
            // About, separator, here: Settings…, separator, Services …
            let _: () = msg_send![app_menu, insertItem: &*item, atIndex: 2isize];
            let _: () = msg_send![app_menu, insertItem: &*separator, atIndex: 3isize];
        }
    }

    pub fn set_settings_title(title: &str) {
        TITLE.with(|t| *t.borrow_mut() = title.to_string());
        ensure();
    }

    pub fn install(on_about: Box<dyn Fn()>, on_settings: Box<dyn Fn()>, settings_title: &str) {
        ABOUT.with(|h| *h.borrow_mut() = Some(on_about));
        SETTINGS.with(|h| *h.borrow_mut() = Some(on_settings));
        TITLE.with(|t| *t.borrow_mut() = settings_title.to_string());
        let Some(cls) = AnyClass::get(c"NSApplication") else { return };
        let imp = |f: ObjFn| unsafe { std::mem::transmute::<ObjFn, Imp>(f) };
        for sel in [sel!(orderFrontStandardAboutPanel:), sel!(orderFrontStandardAboutPanelWithOptions:)] {
            if let Some(m) = cls.instance_method(sel) {
                // same signature (self, _cmd, one object) as the methods it replaces
                unsafe { m.set_implementation(imp(about)) };
            }
        }
        // the menu item's action; with no target it reaches NSApplication through the responder chain
        unsafe { objc2::ffi::class_addMethod(cls as *const AnyClass as *mut AnyClass, sel!(mfcShowSettings:), imp(settings), c"v@:@".as_ptr()) };
        if let Some(m) = cls.instance_method(sel!(setMainMenu:)) {
            let original = unsafe { m.set_implementation(imp(set_main_menu)) };
            ORIGINAL_SET_MAIN_MENU.store(original as usize, Ordering::Relaxed);
        }
        ensure();
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let settings = Rc::new(RefCell::new(load_settings()));
    let app = AppWindow::new()?;
    repaint_on_resize!(app);
    let _ = slint::select_bundled_translation(&settings.borrow().language);
    app.set_language(settings.borrow().language.clone().into());
    apply_menus(&app.global::<Shared>(), &settings.borrow());
    let drives: Drives = Rc::default();
    sync_drives(&app, &drives);
    let others = Others::default();
    let last: Last = Arc::default();
    let renames = Rc::new(RefCell::new(Renames::load()));
    if let Err(e) = &renames.borrow().journal {
        log::write(&format!("journal unreadable: {e}"));
        app.set_error(e.clone().into());
    }
    show_renamed(&app, &renames.borrow());
    log::write(&format!("start {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH));
    std::panic::set_hook(Box::new(|info| log::write(&format!("crash: {info}"))));

    // Menu actions run after the click: they rename menu items, which crashed during macOS menu handling (seen in Ferry).
    app.global::<Shared>().on_select_language({
        let (weak, settings, others) = (app.as_weak(), settings.clone(), others.clone());
        move |i| {
            let (weak, settings, others) = (weak.clone(), settings.clone(), others.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let Some(app) = weak.upgrade() else { return };
                let code = LANGUAGES.get(i as usize).map_or("en", |l| l.0);
                let _ = slint::select_bundled_translation(code);
                app.set_language(code.into());
                settings.borrow_mut().language = code.into();
                save_settings(&settings.borrow());
                apply_all(&app, &others, &settings.borrow());
            });
        }
    });

    app.global::<Shared>().on_select_dark({
        let (weak, settings, others) = (app.as_weak(), settings.clone(), others.clone());
        move |dark| {
            let (weak, settings, others) = (weak.clone(), settings.clone(), others.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let Some(app) = weak.upgrade() else { return };
                settings.borrow_mut().dark = dark;
                save_settings(&settings.borrow());
                apply_all(&app, &others, &settings.borrow());
            });
        }
    });

    app.global::<Shared>().on_show_about({
        let (weak, settings, about) = (app.as_weak(), settings.clone(), others.about.clone());
        move || {
            let (weak, settings, about) = (weak.clone(), settings.clone(), about.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let mut about = about.borrow_mut();
                if about.is_none() {
                    *about = AboutWindow::new().ok();
                    if let Some(a) = about.as_ref() {
                        repaint_on_resize!(a);
                        a.set_version(env!("CARGO_PKG_VERSION").into());
                        a.on_open_source(|| {
                            let _ = std::process::Command::new("open").arg("https://github.com/moritz-poth/mediafiles-check").spawn();
                        });
                        a.on_open_license(|| {
                            // bundled license text, otherwise the FSF page
                            let bundled = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.join("Resources/LICENSE.txt")));
                            let target = bundled.filter(|p| p.exists()).map_or("https://www.gnu.org/licenses/gpl-3.0.html".into(), |p| p.display().to_string());
                            let _ = std::process::Command::new("open").arg(target).spawn();
                        });
                        forward_menu(&a.global::<Shared>(), &weak);
                    }
                }
                if let Some(a) = about.as_ref() {
                    apply_menus(&a.global::<Shared>(), &settings.borrow());
                    let _ = a.show();
                }
            });
        }
    });

    app.global::<Shared>().on_show_settings({
        let (weak, settings, win) = (app.as_weak(), settings.clone(), others.settings.clone());
        move || {
            let (weak, settings, win) = (weak.clone(), settings.clone(), win.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let mut win = win.borrow_mut();
                if win.is_none() {
                    *win = SettingsWindow::new().ok();
                    win.iter().for_each(|w| {
                        repaint_on_resize!(w);
                        forward_menu(&w.global::<Shared>(), &weak)
                    });
                }
                if let Some(w) = win.as_ref() {
                    apply_menus(&w.global::<Shared>(), &settings.borrow());
                    let _ = w.show();
                }
            });
        }
    });

    app.global::<Shared>().on_show_log(|| {
        log::write("log file shown");
        let _ = std::process::Command::new("open").arg("-R").arg(log::path()).spawn();
    });

    app.global::<Shared>().on_show_help({
        let (weak, settings, help) = (app.as_weak(), settings.clone(), others.help.clone());
        move || {
            let (weak, settings, help) = (weak.clone(), settings.clone(), help.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let mut help = help.borrow_mut();
                if help.is_none() {
                    *help = HelpWindow::new().ok();
                    help.iter().for_each(|h| {
                        repaint_on_resize!(h);
                        forward_menu(&h.global::<Shared>(), &weak)
                    });
                }
                if let Some(h) = help.as_ref() {
                    update_help(h, &settings.borrow());
                    let _ = h.show();
                }
            });
        }
    });

    app.on_toggle_drive({
        let (weak, drives) = (app.as_weak(), drives.clone());
        move |i| {
            let mut d = drives.borrow_mut();
            if let Some(x) = d.get_mut(i as usize) {
                x.1 = !x.1;
            }
            show_drives(&weak.unwrap(), &d);
        }
    });

    app.on_check_drives({
        let (weak, last, drives) = (app.as_weak(), last.clone(), drives.clone());
        move || {
            let roots: Vec<PathBuf> = drives.borrow().iter().filter(|d| d.1).map(|d| d.0.clone()).collect();
            start_check(&weak.unwrap(), roots, last.clone());
        }
    });

    app.on_check_folder({
        let (weak, last) = (app.as_weak(), last.clone());
        move || {
            let app = weak.unwrap();
            if let Some(folder) = rfd::FileDialog::new().set_title(app.get_txt_choose_folder()).pick_folder() {
                start_check(&app, vec![folder], last.clone());
            }
        }
    });

    app.on_save_report({
        let (weak, last) = (app.as_weak(), last.clone());
        move || {
            let app = weak.unwrap();
            let lang = app.get_language().to_string();
            let name = format!("MediaFiles Check {}.txt", chrono::Local::now().format("%Y-%m-%d"));
            let Some(target) = rfd::FileDialog::new().set_file_name(name).add_filter("Text", &["txt"]).save_file() else { return };
            if let Err(e) = fs::write(&target, report_text(&last.lock().unwrap().reports, &lang)) {
                app.set_error(format!("{}: {e}", target.display()).into());
                app.set_page(Page::Home);
            }
        }
    });

    app.on_back({
        let (weak, drives, renames) = (app.as_weak(), drives.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            sync_drives(&app, &drives);
            show_renamed(&app, &renames.borrow());
            app.set_page(Page::Home);
        }
    });

    app.on_show_rename({
        let weak = app.as_weak();
        move || {
            let app = weak.unwrap();
            app.set_mc_running(media_composer_running());
            app.set_page(Page::Rename);
        }
    });

    app.on_rename({
        let (weak, renames, last) = (app.as_weak(), renames.clone(), last.clone());
        move || {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let candidates = std::mem::take(&mut last.lock().unwrap().candidates);
            show_candidates(&app, &[]);
            let mut st = renames.borrow_mut();
            let Ok(journal) = st.journal.as_mut() else { return };
            let errors: Vec<NoticeRow> = candidates
                .iter()
                .filter_map(|(folder, to)| {
                    let result = rename::rename_for_avid(folder, to, journal, &journal_path());
                    log::write(&format!("rename {} -> {to}: {}", folder.display(), result.as_ref().map_or_else(|e| e.to_string(), |_| "ok".into())));
                    result.err().map(|e| notice(3, &volume_name(folder), &e.to_string(), 0))
                })
                .collect();
            show_renamed(&app, &st);
            if errors.is_empty() {
                app.set_page(Page::Home);
            } else {
                app.set_notice(ModelRc::new(VecModel::from(errors)));
                app.set_page(Page::Notice);
            }
        }
    });

    app.on_rename_back({
        let (weak, renames) = (app.as_weak(), renames.clone());
        move || {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let mut st = renames.borrow_mut();
            let Ok(journal) = &st.journal else { return };
            let todo = journal.entries.iter().filter(|r| rename::status(r) != Status::Offline).cloned().collect();
            st.back = Some(BackFlow { todo, cur: None, notes: vec![] });
            advance(&app, &mut st);
        }
    });

    let projects_state: Rc<RefCell<ProjectsState>> = Rc::default();
    app.on_open_projects({
        let (weak, win, st, settings) = (app.as_weak(), others.projects.clone(), projects_state.clone(), settings.clone());
        move || {
            let (weak, win, st, settings) = (weak.clone(), win.clone(), st.clone(), settings.clone());
            slint::Timer::single_shot(Duration::ZERO, move || open_projects(&weak, &win, &st, &settings.borrow()));
        }
    });

    app.on_start_media_composer({
        let weak = app.as_weak();
        move || {
            let started = std::process::Command::new("open").args(["-b", "com.avid.mediacomposer"]).status();
            log::write(&format!("start Media Composer: {started:?}"));
            if !started.is_ok_and(|s| s.success()) {
                let app = weak.unwrap();
                app.set_error(app.get_txt_mc_not_started());
            }
        }
    });

    app.on_answer({
        let (weak, renames) = (app.as_weak(), renames.clone());
        move |answer| {
            let app = weak.unwrap();
            let mut st = renames.borrow_mut();
            let Some(back) = st.back.as_mut() else { return };
            let Some(cur) = back.cur.as_mut() else { return };
            let question = cur.questions.pop_front();
            if answer < 0 {
                log::write(&format!("left renamed: {}", cur.r.dir().display()));
                back.notes.push(notice(2, &cur.r.current, "", 0));
                back.cur = None;
            } else if let Some(Question::Added(_)) = question {
                cur.choice = match answer {
                    1 => Added::NewFolder,
                    2 => Added::FirstFolder,
                    _ => Added::Keep,
                };
            }
            advance(&app, &mut st);
        }
    });

    // shortcut: polls /Volumes every 2 s on the start page; a hanging network share could stall it – use DiskArbitration if that happens
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_secs(2), {
        let (weak, drives, renames) = (app.as_weak(), drives.clone(), renames.clone());
        move || {
            let Some(app) = weak.upgrade() else { return };
            #[cfg(target_os = "macos")]
            mac_menu::ensure();
            match app.get_page() {
                Page::Home => {
                    sync_drives(&app, &drives);
                    show_renamed(&app, &renames.borrow());
                }
                Page::Rename => app.set_mc_running(media_composer_running()),
                _ => {}
            }
        }
    });

    #[cfg(target_os = "macos")]
    {
        let (w1, w2) = (app.as_weak(), app.as_weak());
        mac_menu::install(
            Box::new(move || w1.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_about())),
            Box::new(move || w2.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_settings())),
            &app.get_txt_settings_menu(),
        );
    }

    app.run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_names_are_shortened_in_the_middle() {
        assert_eq!(super::shorten("Avid8.1", 10), "Avid8.1");
        assert_eq!(super::shorten("/Volumes/Archive/Projects/Avid Projects", 21), "/Volumes/A…d Projects");
    }

    #[test]
    fn help_is_split_at_headings_in_every_language() {
        for l in super::LANGUAGES {
            let (subtitle, blocks) = super::help_blocks(l.2);
            assert!(!subtitle.is_empty(), "{}", l.0);
            assert!(blocks.iter().filter(|b| !b.heading.is_empty()).count() >= 5, "{}", l.0);
            assert!(!blocks[0].heading.is_empty(), "{}: starts with a heading", l.0);
        }
    }
}
