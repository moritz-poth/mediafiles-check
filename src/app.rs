//! App: check all drives or a chosen folder, show the differences, save them as a text report;
//! rename folders so Media Composer rebuilds their databases and rename them back afterwards.
#![cfg_attr(windows, windows_subsystem = "windows")]

use mediafiles_check::rename::{self, Added, Journal, Renamed, Status};
use mediafiles_check::projects::{self, ClipKind, MissingGroup, ScanResult};
use mediafiles_check::duplicates;
use mediafiles_check::rebalance::{self, Plan, Rebalanced};
use mediafiles_check::report::{self, Outcome, Report};
use mediafiles_check::{FolderReport, UPDATE_APP, check, find_volumes, log, pick, report_text, selftest, update};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    /// settings tab opened by Settings… (0 General, 1 Updates, 2 Support; About is never remembered)
    tab: i32,
    /// "Get test versions": also offer GitHub prereleases (always while a test version runs)
    betas: bool,
    /// Rebalancing: at most this many MXF files per folder
    limit: usize,
    /// not saved; shared with the threads that check and install
    update: Arc<Mutex<Update>>,
}

/// Update state as the windows show it (Shared.update-*).
#[derive(Default)]
struct Update {
    available: Option<update::Release>,
    /// 0 nothing, 1 checking, 2 up to date, 3 installing, 4 failed
    state: i32,
    /// with state 4: 0 check, 1 download, 2 damaged, 3 replace, 4 restart
    error: i32,
    details: String,
}

impl Update {
    fn failed(&mut self, e: update::Error) {
        self.state = 4;
        (self.error, self.details) = match e {
            update::Error::Check => (0, String::new()),
            update::Error::Download => (1, String::new()),
            update::Error::Damaged => (2, String::new()),
            update::Error::Replace(d) => (3, d),
            update::Error::Restart => (4, String::new()),
        };
    }
}

fn app_dir() -> Option<PathBuf> {
    let (var, sub) = if cfg!(windows) { ("APPDATA", "MediaFiles Check") } else { ("HOME", "Library/Application Support/MediaFiles Check") };
    std::env::var_os(var).map(|b| PathBuf::from(b).join(sub))
}

fn settings_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("settings.txt"))
}

/// Which files rebalancing moved where – needed to undo it.
fn rebalance_journal_path() -> PathBuf {
    app_dir().unwrap_or_else(|| PathBuf::from(".")).join("rebalanced.json")
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
    let dark = lines.next() != Some("light");
    let tab = lines.next().and_then(|l| l.parse().ok()).filter(|t| matches!(t, 0..=2 | 4)).unwrap_or(0);
    let betas = lines.next() == Some("betas");
    let limit = lines.next().and_then(|l| l.parse().ok()).filter(|l| rebalance::LIMITS.contains(l)).unwrap_or(rebalance::DEFAULT_LIMIT);
    Settings { language, dark, tab, betas, limit, update: Arc::default() }
}

fn save_settings(s: &Settings) {
    // comfort only; if saving fails the defaults apply next time
    if let Some(p) = settings_path() {
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::write(p, format!("{}\n{}\n{}\n{}\n{}\n", s.language, if s.dark { "dark" } else { "light" }, s.tab, if s.betas { "betas" } else { "final" }, s.limit));
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
    shared.set_betas(s.betas);
    shared.set_limit(s.limit as i32);
    let u = s.update.lock().unwrap();
    shared.set_version(env!("CARGO_PKG_VERSION").into());
    shared.set_update_version(u.available.as_ref().map(|r| r.tag.trim_start_matches('v').to_string()).unwrap_or_default().into());
    shared.set_update_state(u.state);
    shared.set_update_error(u.error);
    shared.set_update_details(u.details.clone().into());
}

/// Asks GitHub for a newer version; `manual` (Settings button) also reports "up to date" and errors.
fn check_for_update(app: &AppWindow, s: &Settings, manual: bool) {
    {
        let mut u = s.update.lock().unwrap();
        if u.state == 1 || u.state == 3 {
            return;
        }
        if manual {
            u.state = 1;
        }
    }
    app.global::<Shared>().invoke_refresh_update();
    let (weak, update, betas) = (app.as_weak(), s.update.clone(), s.betas);
    std::thread::spawn(move || {
        let result = update::check(&UPDATE_APP, env!("CARGO_PKG_VERSION"), betas);
        match &result {
            Ok(Some(r)) => log::write(&format!("update available: {}", r.tag)),
            Ok(None) => {}
            Err(_) => log::write("update check failed"),
        }
        {
            let mut u = update.lock().unwrap();
            if u.state == 3 {
                return;
            }
            match result {
                Ok(r) => {
                    let found = r.is_some();
                    u.available = r;
                    if manual {
                        u.state = if found { 0 } else { 2 };
                    }
                }
                Err(e) if manual => u.failed(e),
                Err(_) => {}
            }
        }
        let _ = slint::invoke_from_event_loop(move || weak.upgrade().iter().for_each(|a| a.global::<Shared>().invoke_refresh_update()));
    });
}

/// Downloads, verifies and swaps in the new version, then quits; the new version starts by itself.
fn install_update(app: &AppWindow, s: &Settings) {
    let release = {
        let mut u = s.update.lock().unwrap();
        let Some(r) = u.available.clone().filter(|_| u.state != 3) else { return };
        u.state = 3;
        r
    };
    log::write(&format!("installing update {}", release.tag));
    app.global::<Shared>().invoke_refresh_update();
    let (weak, update) = (app.as_weak(), s.update.clone());
    std::thread::spawn(move || {
        let result = update::download(&release, &UPDATE_APP).and_then(|new| update::swap_and_restart(&new));
        if let Err(e) = result {
            log::write(&format!("update failed: {e:?}"));
            update.lock().unwrap().failed(e);
            let _ = slint::invoke_from_event_loop(move || weak.upgrade().iter().for_each(|a| a.global::<Shared>().invoke_refresh_update()));
            return;
        }
        log::write("update installed, restarting");
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
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

/// A picture of the manual (`![…](images/<lang>/x.png)`, path relative to docs/help): in the app bundle under
/// Resources/help, when run from the repo in docs/help.
fn help_picture(path: &str) -> slint::Image {
    let bundled = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.join("Resources/help").join(path)));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/help").join(path);
    bundled.filter(|p| p.is_file()).or(Some(repo)).and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default()
}

/// Splits the manual into blocks: "# " = subtitle, "## " = heading, every paragraph its own block
/// (StyledText knows neither headings nor paragraph spacing), a line "![…](path)" a picture. Backticks go, so
/// everything stays in the app font.
fn help_blocks(markdown: &str) -> (String, Vec<HelpBlock>) {
    let mut subtitle = String::new();
    let mut blocks = vec![];
    let mut paragraph = String::new();
    let flush = |paragraph: &mut String, blocks: &mut Vec<HelpBlock>| {
        // "<folder>" would otherwise be an HTML tag
        let text = paragraph.trim().replace('`', "").replace('<', "‹").replace('>', "›");
        if !text.is_empty() {
            let body = slint::StyledText::from_markdown(&text).unwrap_or_else(|_| slint::StyledText::from_plain_text(&text));
            blocks.push(HelpBlock { heading: "".into(), body, picture: Default::default() });
        }
        paragraph.clear();
    };
    for line in markdown.lines() {
        if let Some(t) = line.strip_prefix("# ") {
            subtitle = t.to_uppercase();
        } else if let Some(h) = line.strip_prefix("## ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(HelpBlock { heading: h.into(), body: Default::default(), picture: Default::default() });
        } else if let Some(path) = line.trim().strip_prefix("![").and_then(|l| l.split_once("](")).and_then(|(_, p)| p.strip_suffix(')')) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(HelpBlock { heading: "".into(), body: Default::default(), picture: help_picture(path) });
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
    shared.on_select_limit(move |l| w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_select_limit(l)));
    let w = weak.clone();
    shared.on_select_betas(move |b| w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_select_betas(b)));
    let w = weak.clone();
    shared.on_show_help(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_help()));
    let w = weak.clone();
    shared.on_show_about(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_about()));
    let w = weak.clone();
    shared.on_show_log(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_log()));
    let w = weak.clone();
    shared.on_check_update(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_check_update()));
    let w = weak.clone();
    shared.on_install_update(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_install_update()));
    let w = weak.clone();
    shared.on_show_release(move || w.upgrade().into_iter().for_each(|a| a.global::<Shared>().invoke_show_release()));
}

/// Short message over the settings after sending (success 3 s, failure 8 s – the file name stays readable).
fn toast(w: &slint::Weak<SettingsWindow>, outcome: &Outcome, lang: &str) {
    let (text, ok) = match outcome {
        Outcome::Sent => (pick(lang, ["Sent – thank you!", "Gesendet – danke!", "Envoyé – merci !", "Enviado, ¡gracias!"]).to_string(), true),
        Outcome::Failed(path) if !path.is_empty() => {
            let file = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (pick(lang, [
                "The report could not be sent. It is saved in Downloads as “{}” – please try again later or send us the file.",
                "Der Bericht konnte nicht gesendet werden. Er liegt in „Downloads“ als „{}“ – bitte später noch einmal versuchen oder uns die Datei schicken.",
                "Le rapport n’a pas pu être envoyé. Il est enregistré dans Téléchargements sous « {} » – réessayez plus tard ou envoyez-nous le fichier.",
                "No se pudo enviar el informe. Está guardado en Descargas como «{}»: inténtalo más tarde o envíanos el archivo.",
            ]).replace("{}", &file), false)
        }
        Outcome::Failed(_) => (pick(lang, ["The report could not be sent or saved.", "Der Bericht konnte weder gesendet noch gesichert werden.", "Le rapport n’a pu être ni envoyé ni enregistré.", "No se pudo enviar ni guardar el informe."]).to_string(), false),
    };
    let Some(win) = w.upgrade() else { return };
    win.set_toast_ok(ok);
    win.set_toast_text(text.clone().into());
    win.set_toast_visible(true);
    let w = w.clone();
    slint::Timer::single_shot(Duration::from_secs(if ok { 3 } else { 8 }), move || {
        // slides out; the text stays until the next toast so the box keeps its size while fading
        w.upgrade().into_iter().filter(|w| w.get_toast_text() == text.as_str()).for_each(|w| w.set_toast_visible(false));
    });
}

/// Settings → Support: self-test and problem report (same flow as Mountpoint). After a successful send the page
/// returns to the settings and shows the toast; on failure it stays, so nothing typed is lost.
fn wire_support(w: &SettingsWindow, settings: &Rc<RefCell<Settings>>) {
    let last: Rc<RefCell<Option<Report>>> = Rc::default();
    w.on_open_coffee(|| {
        let _ = std::process::Command::new("open").arg("https://buymeacoffee.com/moritzpoth").spawn();
    });
    w.on_back({
        let weak = w.as_weak();
        move || {
            let Some(w) = weak.upgrade() else { return };
            if w.get_test_phase() == 2 {
                w.set_test_phase(0);
                w.set_test_rows(ModelRc::default());
            }
            w.set_page(0);
        }
    });
    w.on_start_selftest({
        let (weak, settings, last) = (w.as_weak(), settings.clone(), last.clone());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let lang = settings.borrow().language.clone();
            w.set_test_phase(1);
            w.set_test_note("".into());
            w.set_test_rows(ModelRc::default());
            let (weak, last) = (weak.clone(), last.clone());
            let (tx, rx) = std::sync::mpsc::channel::<Report>();
            let l = lang.clone();
            let progress_weak = weak.clone();
            std::thread::spawn(move || {
                let r = selftest::run(&l, &mut |done, next| {
                    let rows: Vec<TestRow> = done.iter().map(|c| TestRow {
                        title: selftest::title(&c.id, &l).into(),
                        detail: c.detail.clone().into(),
                        status: match c.status { "pass" => 0, "fail" => 1, _ => 2 },
                    }).collect();
                    let next = if next.is_empty() { String::new() } else { selftest::title(selftest::CHECKS.iter().find(|c| c.1 == next).map_or(next, |c| c.0), &l) + "…" };
                    let pw = progress_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = pw.upgrade() {
                            w.set_test_rows(ModelRc::new(VecModel::from(rows)));
                            w.set_test_current(next.into());
                        }
                    });
                });
                let _ = tx.send(r);
            });
            // the report itself is not Send through slint's closure in every version: poll the channel
            let timer = Rc::new(slint::Timer::default());
            let t = timer.clone();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
                let Ok(r) = rx.try_recv() else { return };
                t.stop();
                if let Some(w) = weak.upgrade() {
                    let n = |s: &str| r.results.iter().filter(|c| c.status == s).count();
                    w.set_test_summary(pick(&lang, ["{p} passed, {f} failed, {s} skipped", "{p} bestanden, {f} fehlgeschlagen, {s} übersprungen", "{p} réussis, {f} échoués, {s} ignorés", "{p} superadas, {f} fallidas, {s} omitidas"])
                        .replace("{p}", &n("pass").to_string()).replace("{f}", &n("fail").to_string()).replace("{s}", &n("skip").to_string()).into());
                    w.set_test_failed(n("fail") > 0);
                    w.set_test_phase(2);
                }
                *last.borrow_mut() = Some(r);
            });
        }
    });
    w.on_save_selftest({
        let (weak, settings, last) = (w.as_weak(), settings.clone(), last.clone());
        move || {
            let Some(r) = last.borrow().clone() else { return };
            let lang = settings.borrow().language.clone();
            let note = match report::save(&r) {
                Some(p) => {
                    let _ = std::process::Command::new("open").arg("-R").arg(&p).spawn();
                    pick(&lang, ["Saved as “{}” in Downloads.", "Gesichert als „{}“ in „Downloads“.", "Enregistré sous « {} » dans Téléchargements.", "Guardado como «{}» en Descargas."])
                        .replace("{}", &p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                }
                None => pick(&lang, ["The report could not be saved.", "Der Bericht konnte nicht gesichert werden.", "Le rapport n’a pas pu être enregistré.", "No se pudo guardar el informe."]).into(),
            };
            weak.upgrade().into_iter().for_each(|w| w.set_test_note(note.clone().into()));
        }
    });
    let send = move |weak: slint::Weak<SettingsWindow>, lang: String, report: Report, on_sent: Box<dyn Fn(&SettingsWindow)>| {
        let Some(w) = weak.upgrade() else { return };
        w.set_sending(true);
        let (tx, rx) = std::sync::mpsc::channel::<Outcome>();
        std::thread::spawn(move || {
            let _ = tx.send(report::send(&report));
        });
        let timer = Rc::new(slint::Timer::default());
        let t = timer.clone();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
            let Ok(outcome) = rx.try_recv() else { return };
            t.stop();
            if let Some(w) = weak.upgrade() {
                w.set_sending(false);
                if outcome == Outcome::Sent {
                    on_sent(&w);
                    w.set_page(0);
                }
                toast(&weak, &outcome, &lang);
            }
        });
    };
    w.on_send_selftest({
        let (weak, settings, last) = (w.as_weak(), settings.clone(), last.clone());
        move || {
            let Some(r) = last.borrow().clone() else { return };
            send(weak.clone(), settings.borrow().language.clone(), r, Box::new(|w: &SettingsWindow| {
                w.set_test_phase(0);
                w.set_test_rows(ModelRc::default());
            }));
        }
    });
    w.on_send_problem({
        let (weak, settings) = (w.as_weak(), settings.clone());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let lang = settings.borrow().language.clone();
            let r = report::problem(&w.get_report_text(), &w.get_report_contact(), w.get_report_attach(), &lang);
            send(weak.clone(), lang, r, Box::new(|w: &SettingsWindow| {
                w.set_report_text("".into());
                w.set_report_contact("".into());
            }));
        }
    });
}

type Win<T> = Rc<RefCell<Option<T>>>;

/// The windows besides the main one, created when first opened.
#[derive(Clone, Default)]
struct Others {
    help: Win<HelpWindow>,
    settings: Win<SettingsWindow>,
    projects: Win<ProjectsWindow>,
    ama: Win<AmaWindow>,
}

/// Language or appearance changed: every open window follows.
fn apply_all(app: &AppWindow, others: &Others, s: &Settings) {
    apply_menus(&app.global::<Shared>(), s);
    others.settings.borrow().iter().for_each(|w| apply_menus(&w.global::<Shared>(), s));
    others.projects.borrow().iter().for_each(|w| apply_menus(&w.global::<Shared>(), s));
    others.ama.borrow().iter().for_each(|w| apply_menus(&w.global::<Shared>(), s));
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
    /// the AMA window (same as `Others::ama`) and its opened groups
    ama: Win<AmaWindow>,
    ama_open: HashSet<usize>,
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
/// The list of missing media files.
fn show_rows(w: &ProjectsWindow, r: &ScanResult, open: &HashSet<usize>) {
    w.set_rows(ModelRc::new(VecModel::from(project_rows(w, &r.by_bin, open, 0))));
}

/// The AMA window (if open) shows the clips of the last result; texts come from the projects window.
fn show_ama(w: &ProjectsWindow, st: &ProjectsState) {
    if let (Some(a), Some(r)) = (st.ama.borrow().as_ref(), st.result.lock().unwrap().as_ref()) {
        a.set_rows(ModelRc::new(VecModel::from(project_rows(w, &r.ama_by_bin, &st.ama_open, 0))));
    }
}

fn project_rows(w: &ProjectsWindow, by_bin: &BTreeMap<(String, String), Vec<MissingGroup>>, open: &HashSet<usize>, first: usize) -> Vec<MissingRow> {
    const MAX_ROWS: usize = 1000;
    let files = |n: usize| if n == 1 { w.get_txt_one_file() } else { w.get_txt_files_n().replace("{}", &n.to_string()).into() };
    let row = |level, text: String, note: SharedString, open, index| MissingRow { level, text: text.into(), note, open, index };
    let mut rows = vec![];
    let mut index = first;
    for ((project, bin), groups) in by_bin {
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
    st.ama_open.clear();
    // its list belongs to the previous result
    if let Some(a) = st.ama.borrow().as_ref() {
        let _ = a.hide();
    }
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
        log::write(&format!("  {} bins ({} unreadable), {} clips, {} media files missing, {} to relink, {} AMA left out, {} without media left out, {} renders", r.bins, r.failed.len(), r.clips, r.missing.len(), r.relink, r.ama, r.no_media, r.renders));
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
            w.set_ama_covered(r.ama_covered as i32);
            w.set_no_media(r.no_media as i32);
            show_rows(&w, &r, &HashSet::new());
            *result.lock().unwrap() = Some(r);
            w.set_step(2);
        });
    });
}

fn open_projects(weak: &slint::Weak<AppWindow>, win: &Win<ProjectsWindow>, st: &Rc<RefCell<ProjectsState>>, settings: &Rc<RefCell<Settings>>) {
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
                        show_rows(&w, r, &st.open);
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
            w.on_show_ama({
                let (ww, st, weak, settings) = (w.as_weak(), st.clone(), weak.clone(), settings.clone());
                move || {
                    let w = ww.unwrap();
                    let ama = st.borrow().ama.clone();
                    if ama.borrow().is_none() {
                        *ama.borrow_mut() = AmaWindow::new().ok();
                        if let Some(a) = ama.borrow().as_ref() {
                            repaint_on_resize!(a);
                            forward_menu(&a.global::<Shared>(), &weak);
                            a.on_toggle_group({
                                let (ww, st) = (ww.clone(), st.clone());
                                move |i| {
                                    let mut s = st.borrow_mut();
                                    let i = i as usize;
                                    if !s.ama_open.remove(&i) {
                                        s.ama_open.insert(i);
                                    }
                                    show_ama(&ww.unwrap(), &s);
                                }
                            });
                            let aw = a.as_weak();
                            a.on_close(move || { let _ = aw.unwrap().hide(); });
                        }
                    }
                    show_ama(&w, &st.borrow());
                    if let Some(a) = ama.borrow().as_ref() {
                        apply_menus(&a.global::<Shared>(), &settings.borrow());
                        let _ = a.show();
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
        apply_menus(&w.global::<Shared>(), &settings.borrow());
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

/// Rebalancing state: the groups found (and whether ticked), the Media Composer folders still to ask about,
/// the new names given, the groups just done (for "Start").
#[derive(Default)]
struct Rebal {
    plans: Vec<(Plan, bool)>,
    ask: VecDeque<usize>,
    names: HashMap<usize, String>,
    done: Vec<(PathBuf, String)>,
}

/// Media Composer updates folders with these names itself ("1" local, "<computer>.<n>" on network drives).
fn is_mc_folder(mxf: &Path, name: &str, computer: &str) -> bool {
    rename::is_avid_name(name, rename::is_network(mxf), computer)
}

/// Folders renamed for Media Composer right now: (MXF folder, original name).
fn renamed_away(st: &Renames) -> Vec<(PathBuf, String)> {
    st.journal.as_ref().map(|j| j.entries.iter().map(|e| (e.mxf.clone(), e.original.clone())).collect()).unwrap_or_default()
}

/// "Start" for these history entries: set the databases of every changed folder aside and rename those Media
/// Composer does not update itself. Marked done in the journal afterwards; a folder that is renamed already (gone
/// under its name) is left alone, so it can run again after the app stopped half-way.
fn rewrite_runs(st: &mut Renames, which: impl Fn(&Rebalanced) -> bool) -> Vec<NoticeRow> {
    let path = rebalance_journal_path();
    let mut journal = match rebalance::Journal::load(&path) {
        Ok(j) => j,
        Err(e) => return vec![notice(3, "rebalanced.json", &e.to_string(), 0)],
    };
    let (mut rows, mut to_rename) = (vec![], vec![]);
    for r in journal.entries.iter_mut().filter(|r| !r.db_done && which(r)) {
        for name in r.to_rewrite() {
            let folder = r.mxf.join(&name);
            if folder.is_dir() {
                rewrite_original(&folder, &st.computer, &mut rows, &mut to_rename);
            }
        }
        r.db_done = true;
    }
    rows.extend(rename_for_mc(st, &to_rename));
    if let Err(e) = journal.save(&path) {
        rows.push(notice(3, &path.display().to_string(), &e.to_string(), 0));
    }
    rows
}

fn drive_and_folder(folder: &Path) -> String {
    let drive = folder.parent().and_then(Path::parent).and_then(Path::parent).map(volume_name).unwrap_or_default();
    format!("{drive}: {}", volume_name(folder))
}

/// "a, b, c" or "a … z" for long lists
fn short_list(names: &[String]) -> String {
    match names {
        [first, .., last] if names.len() > 3 => format!("{first} … {last}"),
        _ => names.join(", "),
    }
}

fn show_plans(app: &AppWindow, plans: &[(Plan, bool)], lang: &str) {
    let rows: Vec<RebalRow> = plans
        .iter()
        .map(|(p, sel)| {
            let names: Vec<String> = p.folders.iter().map(|f| f.0.clone()).collect();
            let detail = if names.is_empty() {
                pick(lang, [
                    "{count} media files in {g} folders → all back into {main}",
                    "{count} Mediendateien in {g} Ordnern → alle zurück nach {main}",
                    "{count} fichiers média dans {g} dossiers → tous de retour dans {main}",
                    "{count} archivos de medios en {g} carpetas → todos de vuelta a {main}",
                ])
            } else {
                pick(lang, [
                    "{count} media files in {g} folders → {k} rebalanced folders ({names}), {stay} stay in {main} · {m} files move",
                    "{count} Mediendateien in {g} Ordnern → {k} rebalanced-Ordner ({names}), {stay} bleiben in {main} · {m} Dateien werden verschoben",
                    "{count} fichiers média dans {g} dossiers → {k} dossiers rééquilibrés ({names}), {stay} restent dans {main} · {m} fichiers déplacés",
                    "{count} archivos de medios en {g} carpetas → {k} carpetas reequilibradas ({names}), quedan {stay} en {main} · se mueven {m} archivos",
                ])
            }
            .replace("{count}", &p.count.to_string())
            .replace("{g}", &(1 + p.members.len()).to_string())
            .replace("{k}", &names.len().to_string())
            .replace("{names}", &short_list(&names))
            .replace("{stay}", &p.stays.to_string())
            .replace("{main}", &p.main)
            .replace("{m}", &p.moves.len().to_string());
            let note = if p.blocked {
                pick(lang, [
                    "A folder of it is renamed for Media Composer – rename it back first.",
                    "Ein Ordner davon ist für Media Composer umbenannt – erst zurückbenennen.",
                    "Un de ses dossiers est renommé pour Media Composer – renommez-le d’abord à l’inverse.",
                    "Una de sus carpetas está renombrada para Media Composer: primero devuélvele el nombre.",
                ])
            } else if p.mc_folder {
                pick(lang, [
                    "Media Composer's own folder – we recommend renaming it first (you will be asked).",
                    "Ordner von Media Composer selbst – wir empfehlen, ihn vorher umzubenennen (du wirst gefragt).",
                    "Dossier de Media Composer lui-même – nous recommandons de le renommer d’abord (la question vous sera posée).",
                    "Carpeta propia de Media Composer: recomendamos renombrarla antes (se te preguntará).",
                ])
            } else {
                ""
            };
            RebalRow { title: drive_and_folder(&p.mxf.join(&p.main)).into(), detail: detail.into(), note: note.into(), selected: *sel && !p.blocked, blocked: p.blocked }
        })
        .collect();
    app.set_rebalance_selected(plans.iter().filter(|p| p.1 && !p.0.blocked).count() as i32);
    app.set_rebalance_rows(ModelRc::new(VecModel::from(rows)));
}

/// Earlier rebalancing runs (History; each group can be undone).
fn show_rebalanced(app: &AppWindow, lang: &str) {
    let path = rebalance_journal_path();
    let mut journal = rebalance::Journal::load(&path).unwrap_or_default();
    // a drive mounted under another path: follow it (the main folder and its rebalanced folders identify it)
    let mut moved = false;
    for r in journal.entries.iter_mut() {
        if r.relocate() {
            log::write(&format!("rebalanced {}: drive now at {}", r.original, r.mxf.display()));
            moved = true;
        }
    }
    if moved && let Err(e) = journal.save(&path) {
        log::write(&format!("rebalanced.json not saved: {e}"));
    }
    let entries = journal.entries;
    app.set_rebalance_pending(entries.iter().filter(|r| !r.db_done).count() as i32);
    let rows: Vec<RunRow> = entries
        .iter()
        .map(|r| {
            let mut text = pick(lang, [
                "{folder} → {k} rebalanced folders, {n} files moved · {date}",
                "{folder} → {k} rebalanced-Ordner, {n} Dateien verschoben · {date}",
                "{folder} → {k} dossiers rééquilibrés, {n} fichiers déplacés · {date}",
                "{folder} → {k} carpetas reequilibradas, {n} archivos movidos · {date}",
            ])
            .replace("{folder}", &drive_and_folder(&r.mxf.join(&r.original)))
            .replace("{k}", &r.rebalanced_folders().len().to_string())
            .replace("{n}", &r.moved_files().to_string())
            .replace("{date}", &r.date);
            if let Some(old) = &r.renamed_from {
                text += &pick(lang, [" (was {})", " (war {})", " (avant : {})", " (antes {})"]).replace("{}", old);
            }
            RunRow { text: text.into(), db_done: r.db_done }
        })
        .collect();
    app.set_rebalanced(ModelRc::new(VecModel::from(rows)));
}

/// Next Media Composer folder to ask about (rename it first?), or run the rebalancing when all are answered.
fn ask_next(app: &AppWindow, rebal: &Rc<RefCell<Rebal>>, settings: &Rc<RefCell<Settings>>, renames: &Rc<RefCell<Renames>>) {
    let next = rebal.borrow_mut().ask.front().copied();
    match next {
        Some(i) => {
            let main = rebal.borrow().plans[i].0.main.clone();
            app.set_ask_folder(main.into());
            app.set_ask_name("".into());
            app.set_ask_error("".into());
            app.set_page(Page::Askname);
        }
        None => start_rebalance(app, rebal, settings.borrow().limit, renames),
    }
}

/// Lays out the ticked groups anew (fresh from the drive, with the new names) and moves the files.
fn start_rebalance(app: &AppWindow, rebal: &Rc<RefCell<Rebal>>, limit: usize, renames: &Rc<RefCell<Renames>>) {
    if media_composer_running() {
        app.set_mc_running(true);
        app.set_page(Page::Rebalance);
        return;
    }
    let chosen: Vec<(Plan, Option<String>)> = {
        let r = rebal.borrow();
        r.plans.iter().enumerate().filter(|(_, p)| p.1 && !p.0.blocked).map(|(i, p)| (p.0.clone(), r.names.get(&i).cloned())).collect()
    };
    let away = renamed_away(&renames.borrow());
    app.set_work_value(0.0);
    app.set_work_text("".into());
    app.set_page(Page::Working);
    let (tx, rx) = std::sync::mpsc::channel::<(Vec<(Rebalanced, rebalance::Moved)>, Vec<NoticeRow>)>();
    std::thread::spawn(move || {
        let (mut done, mut errors) = (vec![], vec![]);
        let path = rebalance_journal_path();
        match rebalance::Journal::load(&path) {
            Err(e) => errors.push(notice(3, "rebalanced.json", &e.to_string(), 0)),
            Ok(mut journal) => {
                let mut reserved: HashMap<PathBuf, HashSet<String>> = HashMap::new();
                for (p, name) in &chosen {
                    let res = reserved.entry(p.mxf.clone()).or_insert_with(|| away.iter().filter(|a| a.0 == p.mxf).map(|a| mediafiles_check::key(&a.1)).collect());
                    let result = rebalance::plan(&p.mxf, &p.main, rebalance::per_folder(limit), name.as_deref(), p.mc_folder, res).and_then(|fresh| rebalance::execute(&fresh, &mut journal, &path));
                    log::write(&format!("rebalance {} (limit {limit}{}): {}", p.mxf.join(&p.main).display(), name.as_ref().map_or(String::new(), |n| format!(", renamed to {n}")), result.as_ref().map_or_else(|e| e.to_string(), |(e, m)| format!("{} moved, {} skipped, folders {:?}", m.moved, m.skipped.len(), e.folders))));
                    match result {
                        Ok(r) => done.push(r),
                        Err(e) => errors.push(notice(3, &drive_and_folder(&p.mxf.join(&p.main)), &e.to_string(), 0)),
                    }
                }
            }
        }
        let _ = tx.send((done, errors));
    });
    let (timer, weak, rebal) = (Rc::new(slint::Timer::default()), app.as_weak(), rebal.clone());
    let t = timer.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
        let Ok((done, mut errors)) = rx.try_recv() else { return };
        t.stop();
        let Some(app) = weak.upgrade() else { return };
        let lang = app.get_language().to_string();
        show_rebalanced(&app, &lang);
        let mut summary: Vec<String> = vec![];
        for (r, m) in &done {
            summary.push(
                pick(&lang, [
                    "{folder}: {n} media files moved, {k} rebalanced folders.",
                    "{folder}: {n} Mediendateien verschoben, {k} rebalanced-Ordner.",
                    "{folder} : {n} fichiers média déplacés, {k} dossiers rééquilibrés.",
                    "{folder}: {n} archivos de medios movidos, {k} carpetas reequilibradas.",
                ])
                .replace("{folder}", &r.original)
                .replace("{n}", &m.moved.to_string())
                .replace("{k}", &r.folders.len().to_string()),
            );
            if !m.skipped.is_empty() {
                errors.push(notice(7, &short_list(&m.skipped), "", m.skipped.len()));
            }
            errors.extend(m.kept.iter().map(|k| notice(6, k, "", 0)));
        }
        rebal.borrow_mut().done = done.iter().map(|(r, _)| (r.mxf.clone(), r.original.clone())).collect();
        if !errors.is_empty() {
            // something went wrong or was left: show it; the groups that worked are in the History (undo / databases)
            app.set_notice(ModelRc::new(VecModel::from(errors)));
            app.set_page(Page::Notice);
            return;
        }
        app.set_rebalance_summary(summary.join("\n").into());
        app.set_mc_running(media_composer_running());
        app.set_page(Page::Rebalanced);
    });
}

/// Which copies "Set aside" moved where – needed to undo it.
fn dup_journal_path() -> PathBuf {
    app_dir().unwrap_or_else(|| PathBuf::from(".")).join("duplicates.json")
}

/// Duplicates state: what the last search found, which groups are ticked, the run just done (for "Start").
#[derive(Default)]
struct Dups {
    found: duplicates::Found,
    selected: Vec<bool>,
    done: Option<duplicates::Run>,
}

/// at most this many groups on the page (the report lists all; the rest are set aside with the automatic choice)
const DUPS_SHOWN: usize = 300;

fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b.div_ceil(1024)),
    }
}

/// Folders renamed for Media Composer right now (their names on the drive): copies there are not touched.
fn renamed_dirs(st: &Renames) -> Vec<PathBuf> {
    st.journal.as_ref().map(|j| j.entries.iter().map(|e| e.dir()).collect()).unwrap_or_default()
}

fn dup_blocked(g: &duplicates::Group, renamed: &[PathBuf]) -> bool {
    g.copies.iter().any(|c| c.path.parent().is_some_and(|p| renamed.iter().any(|r| r == p)))
}

fn show_dups(app: &AppWindow, d: &Dups, renamed: &[PathBuf], lang: &str) {
    let f = &d.found;
    let mut summary = if f.groups.is_empty() {
        pick(lang, [
            "No duplicates: each of the {f} media files is there only once.",
            "Keine Duplikate: Jede der {f} Mediendateien gibt es nur einmal.",
            "Aucun doublon : chacun des {f} fichiers média n’existe qu’une fois.",
            "Sin duplicados: cada uno de los {f} archivos de medios está una sola vez.",
        ])
        .replace("{f}", &f.files.to_string())
    } else {
        pick(lang, [
            "{g} media files are there more than once ({f} looked at). Setting the ticked copies aside frees {size}.",
            "{g} Mediendateien gibt es mehrfach ({f} angesehen). Die gewählten Kopien zur Seite zu legen macht {size} frei.",
            "{g} fichiers média existent en plusieurs exemplaires ({f} examinés). Mettre de côté les copies choisies libère {size}.",
            "{g} archivos de medios están más de una vez ({f} revisados). Apartar las copias elegidas libera {size}.",
        ])
        .replace("{g}", &f.groups.len().to_string())
        .replace("{f}", &f.files.to_string())
        .replace("{size}", &size_text(f.groups.iter().zip(&d.selected).filter(|(g, s)| **s && !dup_blocked(g, renamed)).map(|(g, _)| g.extra()).sum()))
    };
    if f.groups.len() > DUPS_SHOWN {
        summary += &pick(lang, [" The first {n} are shown; the report lists all.", " Die ersten {n} werden gezeigt, der Bericht listet alle.", " Les {n} premiers sont affichés ; le rapport les liste tous.", " Se muestran los primeros {n}; el informe los lista todos."]).replace("{n}", &DUPS_SHOWN.to_string());
    }
    if !f.mirrors.is_empty() {
        summary += pick(lang, [" Mountpoint workspaces were left out (they show files of another drive).", " Mountpoint-Arbeitsbereiche wurden ausgelassen (sie zeigen Dateien eines anderen Laufwerks).", " Les espaces Mountpoint ont été ignorés (ils montrent les fichiers d’un autre disque).", " Se omitieron los espacios de Mountpoint (muestran archivos de otra unidad)."]);
    }
    if !f.unreadable.is_empty() {
        summary += &pick(lang, [" {n} files could not be read (see report).", " {n} Dateien waren nicht lesbar (siehe Bericht).", " {n} fichiers étaient illisibles (voir le rapport).", " {n} archivos no se pudieron leer (ver informe)."]).replace("{n}", &f.unreadable.len().to_string());
    }
    let rows: Vec<DupGroup> = f
        .groups
        .iter()
        .zip(&d.selected)
        .take(DUPS_SHOWN)
        .map(|(g, sel)| {
            let blocked = dup_blocked(g, renamed);
            let detail = pick(lang, ["{k} copies · {size} each", "{k} Kopien · je {size}", "{k} copies · {size} chacune", "{k} copias · {size} cada una"])
                .replace("{k}", &g.copies.len().to_string())
                .replace("{size}", &size_text(g.copies[g.keep].size));
            let note = if blocked {
                pick(lang, ["A copy is in a folder renamed for Media Composer – rename it back first.", "Eine Kopie liegt in einem für Media Composer umbenannten Ordner – erst zurückbenennen.", "Une copie est dans un dossier renommé pour Media Composer – renommez-le d’abord à l’inverse.", "Una copia está en una carpeta renombrada para Media Composer: primero devuélvele el nombre."])
            } else if g.sizes_differ() {
                pick(lang, ["Same Media Composer ID, but different sizes – check the copies before setting them aside.", "Gleiche Media-Composer-ID, aber unterschiedliche Größen – die Kopien vor dem Zur-Seite-Legen prüfen.", "Même ID Media Composer, mais tailles différentes – vérifiez les copies avant de les mettre de côté.", "Mismo ID de Media Composer, pero tamaños distintos: revisa las copias antes de apartarlas."])
            } else if !g.by_id {
                pick(lang, ["No Media Composer ID – found by name, size and checksum.", "Ohne Media-Composer-ID – gefunden über Name, Größe und Prüfsumme.", "Sans ID Media Composer – trouvé par nom, taille et somme de contrôle.", "Sin ID de Media Composer: encontrado por nombre, tamaño y suma de control."])
            } else {
                ""
            };
            let title = g.name();
            let copies: Vec<DupCopy> = g
                .copies
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let name = c.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let place = c.path.parent().map(drive_and_folder).unwrap_or_default();
                    DupCopy { place: if name == title { place } else { format!("{place} · {name}") }.into(), keep: i == g.keep }
                })
                .collect();
            DupGroup { title: title.into(), detail: detail.into(), note: note.into(), selected: *sel && !blocked, blocked, copies: ModelRc::new(VecModel::from(copies)) }
        })
        .collect();
    app.set_dup_summary(summary.into());
    app.set_dup_selected(f.groups.iter().zip(&d.selected).filter(|(g, s)| **s && !dup_blocked(g, renamed)).count() as i32);
    app.set_dup_groups(ModelRc::new(VecModel::from(rows)));
}

/// Earlier "set aside" runs (History).
fn show_dup_runs(app: &AppWindow, lang: &str) {
    let runs: duplicates::Journal = duplicates::load(&dup_journal_path()).unwrap_or_default();
    app.set_dup_pending(runs.entries.iter().filter(|r| !r.db_done).count() as i32);
    let rows: Vec<RunRow> = runs
        .entries
        .iter()
        .map(|r| {
            let text = pick(lang, ["{n} copies set aside ({size}) · {date}", "{n} Kopien zur Seite gelegt ({size}) · {date}", "{n} copies mises de côté ({size}) · {date}", "{n} copias apartadas ({size}) · {date}"])
                .replace("{n}", &r.moved.len().to_string())
                .replace("{size}", &size_text(r.bytes()))
                .replace("{date}", &r.date);
            RunRow { text: text.into(), db_done: r.db_done }
        })
        .collect();
    app.set_dup_runs(ModelRc::new(VecModel::from(rows)));
}

/// Sets the databases of these folders aside and renames those Media Composer does not update itself.
fn rewrite_folders(st: &mut Renames, folders: &[PathBuf]) -> Vec<NoticeRow> {
    let (mut rows, mut to_rename) = (vec![], vec![]);
    for f in folders.iter().filter(|f| f.is_dir()) {
        rewrite_original(f, &st.computer, &mut rows, &mut to_rename);
    }
    rows.extend(rename_for_mc(st, &to_rename));
    rows
}

/// Renames folders for Media Composer (same as the Rename page) and returns the errors as notice rows.
fn rename_for_mc(st: &mut Renames, folders: &[PathBuf]) -> Vec<NoticeRow> {
    let names = rename::plan_names(folders, &st.computer);
    let Ok(journal) = st.journal.as_mut() else { return vec![notice(3, "renamed.json", "unreadable – nothing renamed", 0)] };
    folders
        .iter()
        .zip(names)
        .filter_map(|(folder, to)| {
            let result = rename::rename_for_avid(folder, &to, journal, &journal_path());
            log::write(&format!("rename {} -> {to}: {}", folder.display(), result.as_ref().map_or_else(|e| e.to_string(), |_| "ok".into())));
            result.err().map(|e| notice(3, &volume_name(folder), &e.to_string(), 0))
        })
        .collect()
}

/// Sets the database of a full folder aside; a folder Media Composer does not update itself is renamed for it too.
fn rewrite_original(folder: &Path, computer: &str, rows: &mut Vec<NoticeRow>, to_rename: &mut Vec<PathBuf>) {
    match rebalance::set_aside_database(folder) {
        Ok(Some(b)) => {
            log::write(&format!("database of {} set aside: {}", folder.display(), b.display()));
            rows.push(notice(8, &volume_name(folder), &b.display().to_string(), 0));
        }
        Ok(None) => {}
        Err(e) => rows.push(notice(3, &volume_name(folder), &e.to_string(), 0)),
    }
    if rename::needs_rename(folder, computer) {
        to_rename.push(folder.to_path_buf());
    }
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

/// Entries that no longer match the drive (moved mount point, app stopped mid-way) – see `rename::repair`.
/// Not while renaming back: that flow holds copies of the entries.
fn repair_renamed(st: &mut Renames) {
    let Ok(journal) = st.journal.as_mut() else { return };
    if st.back.is_some() {
        return;
    }
    let before = journal.entries.len();
    let mut changed = false;
    journal.entries.retain_mut(|r| match rename::repair(r) {
        None => true,
        Some(how) => {
            log::write(&format!("renamed folder {} (was {}): {how:?}", r.dir().display(), r.original));
            changed = true;
            how == rename::Repair::Moved
        }
    });
    if (changed || journal.entries.len() != before) && let Err(e) = journal.save(&journal_path()) {
        log::write(&format!("journal not saved: {e}"));
    }
}

fn show_renamed(app: &AppWindow, st: &mut Renames) {
    repair_renamed(st);
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

/// AppKit details Slint does not offer: colours in sRGB like the Swift apps, and the settings window's title bar.
#[cfg(target_os = "macos")]
mod mac_window {
    use objc2::encode::{Encode, Encoding};
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::{class, msg_send, sel};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size {
        width: f64,
        height: f64,
    }
    unsafe impl Encode for Size {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    unsafe impl Encode for Rect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]), Size::ENCODING]);
    }

    /// Usable height of the screen with the key window (without menu bar and Dock), in points; 0 if unknown.
    pub fn screen_height() -> f64 {
        unsafe {
            let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
            if screen.is_null() {
                return 0.0;
            }
            let frame: Rect = msg_send![screen, visibleFrame];
            frame.height
        }
    }

    type Init = extern "C-unwind" fn(*mut AnyObject, Sel, Rect, usize, usize, bool) -> *mut AnyObject;
    static ORIGINAL_INIT: AtomicUsize = AtomicUsize::new(0);

    extern "C-unwind" fn init_srgb(this: *mut AnyObject, sel: Sel, rect: Rect, style: usize, backing: usize, defer: bool) -> *mut AnyObject {
        let original: Init = unsafe { std::mem::transmute(ORIGINAL_INIT.load(Ordering::Relaxed)) };
        let win = original(this, sel, rect, style, backing, defer);
        if !win.is_null() {
            unsafe {
                let srgb: *mut AnyObject = msg_send![class!(NSColorSpace), sRGBColorSpace];
                let _: () = msg_send![win, setColorSpace: srgb];
            }
        }
        win
    }

    /// Slint (femtovg, OpenGL) draws without colour management, so #1f2225 came out as #21262a next to Mountpoint's
    /// exact #1f2225. A window tagged as sRGB before its first frame shows the Theme colours exactly like the SwiftUI
    /// apps; set later it only worked now and then – so every NSWindow gets sRGB right in its initializer.
    /// Call once, before the first window is created.
    pub fn srgb_windows() {
        let cls = class!(NSWindow);
        if let Some(m) = cls.instance_method(sel!(initWithContentRect:styleMask:backing:defer:)) {
            let imp: Imp = unsafe { std::mem::transmute(init_srgb as Init) };
            let original = unsafe { m.set_implementation(imp) };
            ORIGINAL_INIT.store(original as usize, Ordering::Relaxed);
        }
    }

    type KeyFn = extern "C-unwind" fn(*mut AnyObject, Sel);
    static ORIGINAL_BECOME_KEY: AtomicUsize = AtomicUsize::new(0);
    static ORIGINAL_RESIGN_KEY: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static KEY_CHANGED: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
    }

    fn key_changed(original: &AtomicUsize, this: *mut AnyObject, sel: Sel) {
        let original: KeyFn = unsafe { std::mem::transmute(original.load(Ordering::Relaxed)) };
        original(this, sel);
        KEY_CHANGED.with(|f| f.borrow().iter().for_each(|f| f()));
    }
    extern "C-unwind" fn become_key(this: *mut AnyObject, sel: Sel) {
        key_changed(&ORIGINAL_BECOME_KEY, this, sel);
    }
    extern "C-unwind" fn resign_key(this: *mut AnyObject, sel: Sel) {
        key_changed(&ORIGINAL_RESIGN_KEY, this, sel);
    }

    /// Calls `f` whenever any window becomes or stops being the key window – the tab bar then changes colour at once
    /// together with the title bar (polling made it lag behind).
    pub fn on_key_change(f: Box<dyn Fn()>) {
        KEY_CHANGED.with(|k| *k.borrow_mut() = Some(f));
        let cls = class!(NSWindow);
        for (name, imp, original) in [
            (sel!(becomeKeyWindow), become_key as KeyFn, &ORIGINAL_BECOME_KEY),
            (sel!(resignKeyWindow), resign_key as KeyFn, &ORIGINAL_RESIGN_KEY),
        ] {
            if original.load(Ordering::Relaxed) != 0 {
                continue;
            }
            if let Some(m) = cls.instance_method(name) {
                let imp: Imp = unsafe { std::mem::transmute(imp) };
                original.store(unsafe { m.set_implementation(imp) } as usize, Ordering::Relaxed);
            }
        }
    }

    fn ns_window(w: &slint::Window) -> Option<*mut AnyObject> {
        let handle = w.window_handle();
        let RawWindowHandle::AppKit(h) = handle.window_handle().ok()?.as_raw() else { return None };
        let view = h.ns_view.as_ptr() as *mut AnyObject;
        let win: *mut AnyObject = unsafe { msg_send![view, window] };
        (!win.is_null()).then_some(win)
    }

    /// Content up to the top edge under a transparent title bar (like an NSToolbar window), so the tab bar and the
    /// title bar are one surface without the separator line. Keeps the content size (`height` = whole window).
    pub fn unify_titlebar(w: &slint::Window, width: f64, height: f64) {
        let Some(win) = ns_window(w) else { return };
        unsafe {
            let mask: usize = msg_send![win, styleMask];
            const FULL_SIZE_CONTENT_VIEW: usize = 1 << 15;
            const MINIATURIZABLE: usize = 1 << 2;
            if mask & FULL_SIZE_CONTENT_VIEW == 0 {
                // HIG settings: minimize and zoom dimmed (zoom already is: the size is fixed)
                let _: () = msg_send![win, setStyleMask: (mask | FULL_SIZE_CONTENT_VIEW) & !MINIATURIZABLE];
                let _: () = msg_send![win, setTitlebarAppearsTransparent: true];
                let _: () = msg_send![win, setContentSize: Size { width, height }];
            }
        }
    }

    /// Title bar, traffic lights and title text in the app's own light/dark (not the system's): every window.
    pub fn appearance_all(dark: bool) {
        let name: &std::ffi::CStr = if dark { c"NSAppearanceNameDarkAqua" } else { c"NSAppearanceNameAqua" };
        unsafe {
            let name: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
            let wanted: *mut AnyObject = msg_send![class!(NSAppearance), appearanceNamed: name];
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let windows: *mut AnyObject = msg_send![app, windows];
            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                let current: *mut AnyObject = msg_send![w, appearance];
                if current != wanted {
                    let _: () = msg_send![w, setAppearance: wanted];
                }
            }
        }
    }

    /// Whether the window is the key window (the title bar is lighter then; the tab bar follows).
    pub fn is_key(w: &slint::Window) -> bool {
        ns_window(w).is_some_and(|win| unsafe { msg_send![win, isKeyWindow] })
    }
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(target_os = "macos")]
    mac_window::srgb_windows();
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
    show_renamed(&app, &mut renames.borrow_mut());
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

    // "Get test versions": the old result no longer applies – hide "up to date" and check again quietly
    app.global::<Shared>().on_select_betas({
        let (weak, settings, others) = (app.as_weak(), settings.clone(), others.clone());
        move |betas| {
            let (weak, settings, others) = (weak.clone(), settings.clone(), others.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let Some(app) = weak.upgrade() else { return };
                settings.borrow_mut().betas = betas;
                save_settings(&settings.borrow());
                {
                    let s = settings.borrow();
                    let mut u = s.update.lock().unwrap();
                    if u.state == 2 || u.state == 4 {
                        u.state = 0;
                    }
                }
                apply_all(&app, &others, &settings.borrow());
                check_for_update(&app, &settings.borrow(), false);
            });
        }
    });

    // Settings… opens the tab chosen last (never About); About opens the same window on the About tab
    let open_settings = {
        let (weak, settings, win) = (app.as_weak(), settings.clone(), others.settings.clone());
        move |about: bool| {
            let (weak, settings, win) = (weak.clone(), settings.clone(), win.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                let mut win = win.borrow_mut();
                if win.is_none() {
                    *win = SettingsWindow::new().ok();
                    win.iter().for_each(|w| {
                        repaint_on_resize!(w);
                        forward_menu(&w.global::<Shared>(), &weak);
                        wire_support(w, &settings);
                        w.set_version(env!("CARGO_PKG_VERSION").into());
                        // HIG settings: restore the most recently viewed pane (also after a restart)
                        w.set_last_tab(settings.borrow().tab);
                        w.on_tab_chosen({
                            let settings = settings.clone();
                            move |tab| {
                                settings.borrow_mut().tab = tab;
                                save_settings(&settings.borrow());
                            }
                        });
                        w.on_open_source(|| {
                            let _ = std::process::Command::new("open").arg("https://github.com/moritz-poth/mediafiles-check").spawn();
                        });
                        w.on_open_license(|| {
                            // bundled license text, otherwise the FSF page
                            let bundled = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.join("Resources/LICENSE.txt")));
                            let target = bundled.filter(|p| p.exists()).map_or("https://www.gnu.org/licenses/gpl-3.0.html".into(), |p| p.display().to_string());
                            let _ = std::process::Command::new("open").arg(target).spawn();
                        });
                    });
                }
                if let Some(w) = win.as_ref() {
                    apply_menus(&w.global::<Shared>(), &settings.borrow());
                    w.set_tab(if about { 3 } else { w.get_last_tab() });
                    let _ = w.show();
                }
            });
        }
    };
    app.global::<Shared>().on_show_settings({
        let open = open_settings.clone();
        move || open(false)
    });
    app.global::<Shared>().on_show_about(move || open_settings(true));

    app.global::<Shared>().on_refresh_update({
        let (weak, settings, others) = (app.as_weak(), settings.clone(), others.clone());
        move || weak.upgrade().iter().for_each(|a| apply_all(a, &others, &settings.borrow()))
    });

    app.global::<Shared>().on_check_update({
        let (weak, settings) = (app.as_weak(), settings.clone());
        move || weak.upgrade().iter().for_each(|a| check_for_update(a, &settings.borrow(), true))
    });
    app.global::<Shared>().on_install_update({
        let (weak, settings) = (app.as_weak(), settings.clone());
        move || weak.upgrade().iter().for_each(|a| install_update(a, &settings.borrow()))
    });
    app.global::<Shared>().on_show_release({
        let settings = settings.clone();
        move || {
            if let Some(r) = settings.borrow().update.lock().unwrap().available.as_ref() {
                let _ = std::process::Command::new("open").arg(&r.page).spawn();
            }
        }
    });
    // quietly shortly after the start, then every 6 hours
    slint::Timer::single_shot(Duration::from_secs(5), {
        let (weak, settings) = (app.as_weak(), settings.clone());
        move || weak.upgrade().iter().for_each(|a| check_for_update(a, &settings.borrow(), false))
    });
    let update_timer = slint::Timer::default();
    update_timer.start(slint::TimerMode::Repeated, Duration::from_secs(6 * 3600), {
        let (weak, settings) = (app.as_weak(), settings.clone());
        move || weak.upgrade().iter().for_each(|a| check_for_update(a, &settings.borrow(), false))
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
                app.invoke_home();
            }
        }
    });

    app.on_back({
        let (weak, drives, renames) = (app.as_weak(), drives.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            sync_drives(&app, &drives);
            show_renamed(&app, &mut renames.borrow_mut());
            app.invoke_home();
        }
    });


    // ── Duplicates ─────────────────────────────────────────
    let dups: Rc<RefCell<Dups>> = Rc::default();
    show_dup_runs(&app, &settings.borrow().language);

    app.on_find_dups({
        let (weak, drives, dups, renames) = (app.as_weak(), drives.clone(), dups.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            let roots: Vec<PathBuf> = drives.borrow().iter().filter(|d| d.1).map(|d| d.0.clone()).collect();
            app.set_work_value(0.0);
            app.set_work_text("".into());
            app.set_page(Page::Working);
            log::write(&format!("duplicates: looking through {}", roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")));
            let (tx, rx) = std::sync::mpsc::channel::<duplicates::Found>();
            let progress = app.as_weak();
            std::thread::spawn(move || {
                let found = duplicates::find(&roots, &mut |i, n, place| {
                    let (place, w) = (place.to_string(), progress.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(a) = w.upgrade() {
                            a.set_work_text(place.into());
                            a.set_work_value(i as f32 / n.max(1) as f32);
                        }
                    });
                });
                let _ = tx.send(found);
            });
            let (timer, weak, dups, renames) = (Rc::new(slint::Timer::default()), weak.clone(), dups.clone(), renames.clone());
            let t = timer.clone();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
                let Ok(found) = rx.try_recv() else { return };
                t.stop();
                let Some(app) = weak.upgrade() else { return };
                log::write(&format!("duplicates: {} files, {} with copies, {} unreadable, {} workspaces left out", found.files, found.groups.len(), found.unreadable.len(), found.mirrors.len()));
                let mut d = dups.borrow_mut();
                // copies with different sizes are never ticked by themselves
                d.selected = found.groups.iter().map(|g| !g.sizes_differ()).collect();
                d.found = found;
                show_dups(&app, &d, &renamed_dirs(&renames.borrow()), &app.get_language());
                app.set_mc_running(media_composer_running());
                app.set_page(Page::Dups);
            });
        }
    });

    app.on_toggle_dup({
        let (weak, dups, renames) = (app.as_weak(), dups.clone(), renames.clone());
        move |i| {
            let app = weak.unwrap();
            let mut d = dups.borrow_mut();
            if let Some(s) = d.selected.get_mut(i as usize) {
                *s = !*s;
            }
            show_dups(&app, &d, &renamed_dirs(&renames.borrow()), &app.get_language());
        }
    });

    app.on_keep_copy({
        let (weak, dups, renames) = (app.as_weak(), dups.clone(), renames.clone());
        move |g, c| {
            let app = weak.unwrap();
            let mut d = dups.borrow_mut();
            if let Some(group) = d.found.groups.get_mut(g as usize).filter(|gr| (c as usize) < gr.copies.len()) {
                group.keep = c as usize;
            }
            show_dups(&app, &d, &renamed_dirs(&renames.borrow()), &app.get_language());
        }
    });

    app.on_save_dups({
        let (weak, dups) = (app.as_weak(), dups.clone());
        move || {
            let app = weak.unwrap();
            let name = format!("MediaFiles Check Duplicates {}.txt", chrono::Local::now().format("%Y-%m-%d"));
            let Some(target) = rfd::FileDialog::new().set_file_name(name).add_filter("Text", &["txt"]).save_file() else { return };
            if let Err(e) = fs::write(&target, duplicates::report_text(&dups.borrow().found, &app.get_language())) {
                app.set_notice(ModelRc::new(VecModel::from(vec![notice(3, &target.display().to_string(), &e.to_string(), 0)])));
                app.set_page(Page::Notice);
            }
        }
    });

    app.on_set_aside_dups({
        let (weak, dups, renames) = (app.as_weak(), dups.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let renamed = renamed_dirs(&renames.borrow());
            let copies: Vec<duplicates::Copy> = {
                let d = dups.borrow();
                d.found
                    .groups
                    .iter()
                    .zip(&d.selected)
                    .filter(|(g, s)| **s && !dup_blocked(g, &renamed))
                    .flat_map(|(g, _)| g.copies.iter().enumerate().filter(|(i, _)| *i != g.keep).map(|(_, c)| c.clone()))
                    .collect()
            };
            app.set_work_value(0.0);
            app.set_work_text("".into());
            app.set_page(Page::Working);
            let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<(duplicates::Run, duplicates::Moved)>>();
            std::thread::spawn(move || {
                let _ = tx.send(duplicates::set_aside(&copies, &dup_journal_path()));
            });
            let (timer, weak, dups) = (Rc::new(slint::Timer::default()), weak.clone(), dups.clone());
            let t = timer.clone();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
                let Ok(result) = rx.try_recv() else { return };
                t.stop();
                let Some(app) = weak.upgrade() else { return };
                let lang = app.get_language().to_string();
                show_dup_runs(&app, &lang);
                match result {
                    Err(e) => {
                        log::write(&format!("duplicates: set aside failed: {e}"));
                        app.set_notice(ModelRc::new(VecModel::from(vec![notice(3, "duplicates", &e.to_string(), 0)])));
                        app.set_page(Page::Notice);
                    }
                    Ok((run, m)) => {
                        log::write(&format!("duplicates: {} copies set aside, {} skipped", m.moved, m.skipped.len()));
                        let mut lines = vec![pick(&lang, [
                            "{n} copies set aside ({size}) into “MediaFiles Check Duplicates” on their drives.",
                            "{n} Kopien zur Seite gelegt ({size}), in „MediaFiles Check Duplicates“ auf ihren Laufwerken.",
                            "{n} copies mises de côté ({size}) dans « MediaFiles Check Duplicates » sur leurs disques.",
                            "{n} copias apartadas ({size}) en «MediaFiles Check Duplicates» en sus unidades.",
                        ])
                        .replace("{n}", &m.moved.to_string())
                        .replace("{size}", &size_text(run.bytes()))];
                        if !m.skipped.is_empty() {
                            lines.push(pick(&lang, ["Left where they are (changed or gone meanwhile): {}", "Liegen geblieben (inzwischen geändert oder weg): {}", "Laissées en place (modifiées ou disparues entre-temps) : {}", "Se quedaron (cambiadas o desaparecidas entretanto): {}"]).replace("{}", &m.skipped.join(", ")));
                        }
                        lines.extend(run.changed.iter().map(|f| format!("· {}", drive_and_folder(f))));
                        dups.borrow_mut().done = Some(run);
                        app.set_dup_done_summary(lines.join("\n").into());
                        app.set_mc_running(media_composer_running());
                        app.set_page(Page::Dupsdone);
                    }
                }
            });
        }
    });

    // "Start" (-1 = the run just done) or "Rewrite databases" of one History entry
    app.on_dup_db({
        let (weak, dups, renames) = (app.as_weak(), dups.clone(), renames.clone());
        move |i| {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let path = dup_journal_path();
            let mut rows = vec![];
            match duplicates::load::<duplicates::Journal>(&path) {
                Err(e) => rows.push(notice(3, "duplicates.json", &e.to_string(), 0)),
                Ok(mut journal) => {
                    let done = dups.borrow_mut().done.take();
                    let index = usize::try_from(i).ok().or_else(|| done.and_then(|d| journal.entries.iter().position(|r| *r == d)));
                    if let Some(run) = index.and_then(|i| journal.entries.get_mut(i)) {
                        rows = rewrite_folders(&mut renames.borrow_mut(), &run.changed);
                        run.db_done = true;
                        if let Err(e) = duplicates::save(&journal, &path) {
                            rows.push(notice(3, &path.display().to_string(), &e.to_string(), 0));
                        }
                    }
                }
            }
            show_renamed(&app, &mut renames.borrow_mut());
            show_dup_runs(&app, &app.get_language());
            app.set_notice(ModelRc::new(VecModel::from(rows)));
            app.set_page(Page::Notice);
        }
    });

    app.on_undo_dup({
        let (weak, renames) = (app.as_weak(), renames.clone());
        move |i| {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let lang = app.get_language().to_string();
            let path = dup_journal_path();
            let mut rows = vec![];
            match duplicates::load::<duplicates::Journal>(&path) {
                Err(e) => rows.push(notice(3, "duplicates.json", &e.to_string(), 0)),
                Ok(mut journal) => {
                    let Some(run) = journal.entries.get(i as usize).cloned() else { return };
                    match duplicates::undo(&run) {
                        Err(e) => rows.push(notice(3, "duplicates", &e.to_string(), 0)),
                        Ok(u) => {
                            log::write(&format!("duplicates: undo, {} back, {} skipped", u.moved, u.skipped.len()));
                            rows.push(notice(5, pick(&lang, ["their folders", "ihre Ordner", "leurs dossiers", "sus carpetas"]), "", u.moved));
                            if !u.skipped.is_empty() {
                                rows.push(notice(7, &short_list(&u.skipped), "", u.skipped.len()));
                            }
                            journal.entries.remove(i as usize);
                            if let Err(e) = duplicates::save(&journal, &path) {
                                rows.push(notice(3, &path.display().to_string(), &e.to_string(), 0));
                            }
                            // the folders got their files back: their databases are rewritten again
                            rows.extend(rewrite_folders(&mut renames.borrow_mut(), &run.changed));
                        }
                    }
                }
            }
            show_renamed(&app, &mut renames.borrow_mut());
            show_dup_runs(&app, &lang);
            app.set_notice(ModelRc::new(VecModel::from(rows)));
            app.set_page(Page::Notice);
        }
    });

    // ── Rebalancing ────────────────────────────────────────
    let rebal: Rc<RefCell<Rebal>> = Rc::default();
    show_rebalanced(&app, &settings.borrow().language);

    app.global::<Shared>().on_select_limit({
        let (weak, settings, others) = (app.as_weak(), settings.clone(), others.clone());
        move |limit| {
            let Some(app) = weak.upgrade() else { return };
            let limit = limit as usize;
            if !rebalance::LIMITS.contains(&limit) {
                return;
            }
            settings.borrow_mut().limit = limit;
            save_settings(&settings.borrow());
            apply_all(&app, &others, &settings.borrow());
        }
    });

    app.on_find_full({
        let (weak, drives, settings, rebal, renames) = (app.as_weak(), drives.clone(), settings.clone(), rebal.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            let roots: Vec<PathBuf> = drives.borrow().iter().filter(|d| d.1).map(|d| d.0.clone()).collect();
            let limit = settings.borrow().limit;
            let (away, computer) = { let st = renames.borrow(); (renamed_away(&st), st.computer.clone()) };
            app.set_work_value(0.0);
            app.set_work_text("".into());
            app.set_page(Page::Working);
            log::write(&format!("rebalance: looking for groups that do not match {limit} files"));
            let (tx, rx) = std::sync::mpsc::channel::<Vec<Plan>>();
            std::thread::spawn(move || {
                let _ = tx.send(rebalance::full_folders(&roots, limit, &away, &|mxf, name| is_mc_folder(mxf, name, &computer)));
            });
            let (timer, weak, rebal) = (Rc::new(slint::Timer::default()), weak.clone(), rebal.clone());
            let t = timer.clone();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
                let Ok(plans) = rx.try_recv() else { return };
                t.stop();
                let Some(app) = weak.upgrade() else { return };
                log::write(&format!("rebalance: {} groups to lay out", plans.len()));
                let mut r = rebal.borrow_mut();
                r.plans = plans.into_iter().map(|p| (p, true)).collect();
                show_plans(&app, &r.plans, &app.get_language());
                app.set_mc_running(media_composer_running());
                app.set_page(Page::Rebalance);
            });
        }
    });

    app.on_toggle_rebalance({
        let (weak, rebal) = (app.as_weak(), rebal.clone());
        move |i| {
            let app = weak.unwrap();
            let mut r = rebal.borrow_mut();
            if let Some(p) = r.plans.get_mut(i as usize) {
                p.1 = !p.1;
            }
            show_plans(&app, &r.plans, &app.get_language());
        }
    });

    // "Rebalance": first ask about every ticked Media Composer folder (rename it?), then run
    app.on_rebalance({
        let (weak, rebal, settings, renames) = (app.as_weak(), rebal.clone(), settings.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            {
                let mut r = rebal.borrow_mut();
                r.names.clear();
                r.ask = r.plans.iter().enumerate().filter(|(_, p)| p.1 && !p.0.blocked && p.0.mc_folder).map(|(i, _)| i).collect();
            }
            ask_next(&app, &rebal, &settings, &renames);
        }
    });

    app.on_rename_main({
        let (weak, rebal, settings, renames) = (app.as_weak(), rebal.clone(), settings.clone(), renames.clone());
        move |name| {
            let app = weak.unwrap();
            let lang = app.get_language().to_string();
            let computer = renames.borrow().computer.clone();
            let problem = {
                let r = rebal.borrow();
                let Some(&i) = r.ask.front() else { return };
                let mxf = &r.plans[i].0.mxf;
                // names given to other groups in the same MXF folder are taken too
                let taken: HashSet<String> = r.names.iter().filter(|(j, _)| r.plans[**j].0.mxf == *mxf).map(|(_, n)| mediafiles_check::key(n)).collect();
                rebalance::check_name(mxf, &name, is_mc_folder(mxf, &name, &computer), &taken)
            };
            use rebalance::NameProblem as N;
            let text = match problem {
                None => {
                    let mut r = rebal.borrow_mut();
                    if let Some(i) = r.ask.pop_front() {
                        r.names.insert(i, name.to_string());
                    }
                    drop(r);
                    ask_next(&app, &rebal, &settings, &renames);
                    return;
                }
                Some(N::Empty) => pick(&lang, ["Enter a name.", "Bitte einen Namen eingeben.", "Saisissez un nom.", "Escribe un nombre."]),
                Some(N::BadCharacters) => pick(&lang, ["Not allowed: “/”, “:”, a leading dot or spaces at the ends.", "Nicht erlaubt: „/“, „:“, ein Punkt am Anfang oder Leerzeichen an den Enden.", "Interdit : « / », « : », un point au début ou des espaces aux extrémités.", "No se permite: «/», «:», un punto al principio ni espacios en los extremos."]),
                Some(N::Exists) => pick(&lang, ["A folder with this name exists already.", "Einen Ordner mit diesem Namen gibt es schon.", "Un dossier de ce nom existe déjà.", "Ya existe una carpeta con este nombre."]),
                Some(N::MediaComposer) => pick(&lang, ["Media Composer uses this name itself – choose another one.", "Diesen Namen verwendet Media Composer selbst – bitte einen anderen wählen.", "Media Composer utilise lui-même ce nom – choisissez-en un autre.", "Media Composer usa este nombre: elige otro."]),
                Some(N::Reserved) => pick(&lang, ["Names with “-rebalanced-” are reserved for rebalanced folders.", "Namen mit „-rebalanced-“ sind für rebalanced-Ordner reserviert.", "Les noms avec « -rebalanced- » sont réservés aux dossiers rééquilibrés.", "Los nombres con «-rebalanced-» están reservados para las carpetas reequilibradas."]),
            };
            app.set_ask_error(text.into());
        }
    });

    app.on_keep_main({
        let (weak, rebal, settings, renames) = (app.as_weak(), rebal.clone(), settings.clone(), renames.clone());
        move || {
            let app = weak.unwrap();
            rebal.borrow_mut().ask.pop_front();
            ask_next(&app, &rebal, &settings, &renames);
        }
    });

    // "Start" (-1 = the groups just done) or "Rewrite databases" of one History entry
    app.on_rebalance_db({
        let (weak, rebal, renames) = (app.as_weak(), rebal.clone(), renames.clone());
        move |i| {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let done = std::mem::take(&mut rebal.borrow_mut().done);
            let journal = rebalance::Journal::load(&rebalance_journal_path()).map(|j| j.entries).unwrap_or_default();
            let pick: Option<&Rebalanced> = usize::try_from(i).ok().and_then(|i| journal.get(i));
            let mut st = renames.borrow_mut();
            let rows = rewrite_runs(&mut st, |r| match pick {
                Some(p) => r.mxf == p.mxf && r.original == p.original,
                None => done.iter().any(|d| d.0 == r.mxf && d.1 == r.original),
            });
            show_renamed(&app, &mut st);
            show_rebalanced(&app, &app.get_language());
            app.set_notice(ModelRc::new(VecModel::from(rows)));
            app.set_page(Page::Notice);
        }
    });

    app.on_undo_rebalance({
        let (weak, renames) = (app.as_weak(), renames.clone());
        move |i| {
            let app = weak.unwrap();
            if media_composer_running() {
                app.set_mc_running(true);
                return;
            }
            let path = rebalance_journal_path();
            let mut journal = match rebalance::Journal::load(&path) {
                Ok(j) => j,
                Err(e) => {
                    app.set_notice(ModelRc::new(VecModel::from(vec![notice(3, "rebalanced.json", &e.to_string(), 0)])));
                    app.set_page(Page::Notice);
                    return;
                }
            };
            let Some(entry) = journal.entries.get(i as usize).cloned() else { return };
            let main = entry.mxf.join(&entry.original);
            let mut rows = vec![];
            match rebalance::undo(&entry) {
                Ok(u) => {
                    log::write(&format!("undo rebalance of {}: {} back, {} skipped, kept {:?}, renamed back {:?}", main.display(), u.moved_back, u.skipped.len(), u.kept, u.renamed_back));
                    rows.push(notice(5, &entry.original, "", u.moved_back));
                    if !u.skipped.is_empty() {
                        rows.push(notice(7, &short_list(&u.skipped), "", u.skipped.len()));
                    }
                    rows.extend(u.kept.iter().map(|k| notice(6, k, "", 0)));
                    if let Some(old) = &u.renamed_back {
                        rows.push(notice(0, &entry.original, old, 0));
                    }
                    journal.entries.remove(i as usize);
                    if let Err(e) = journal.save(&path) {
                        rows.push(notice(3, &path.display().to_string(), &e.to_string(), 0));
                    }
                    // the main folder's database was set aside: a folder Media Composer does not update gets renamed
                    let main = entry.mxf.join(u.renamed_back.as_deref().unwrap_or(&entry.original));
                    let mut st = renames.borrow_mut();
                    if rename::needs_rename(&main, &st.computer) {
                        rows.extend(rename_for_mc(&mut st, std::slice::from_ref(&main)));
                    }
                    show_renamed(&app, &mut st);
                }
                Err(e) => rows.push(notice(3, &drive_and_folder(&main), &e.to_string(), 0)),
            }
            show_rebalanced(&app, &app.get_language());
            app.set_notice(ModelRc::new(VecModel::from(rows)));
            app.set_page(Page::Notice);
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
            show_renamed(&app, &mut st);
            if errors.is_empty() {
                app.set_page(Page::Maintain);
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
    projects_state.borrow_mut().ama = others.ama.clone();
    app.on_open_projects({
        let (weak, win, st, settings) = (app.as_weak(), others.projects.clone(), projects_state.clone(), settings.clone());
        move || {
            let (weak, win, st, settings) = (weak.clone(), win.clone(), st.clone(), settings.clone());
            slint::Timer::single_shot(Duration::ZERO, move || open_projects(&weak, &win, &st, &settings));
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
                Page::Home | Page::Maintain | Page::Rebalstart | Page::Dupsstart | Page::Runs | Page::Exchange | Page::Projects | Page::Tasks => {
                    sync_drives(&app, &drives);
                    show_renamed(&app, &mut renames.borrow_mut());
                    show_rebalanced(&app, &app.get_language());
                    show_dup_runs(&app, &app.get_language());
                }
                _ => {}
            }
            if app.get_page() != Page::Working {
                app.set_mc_running(media_composer_running());
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

    // title bars follow the app's light/dark; the settings tab bar follows the title bar (lighter while active)
    // shortcut: polls every 200 ms for the title bar style and appearance – cheap, both only change something once
    #[cfg(target_os = "macos")]
    let chrome = slint::Timer::default();
    #[cfg(target_os = "macos")]
    chrome.start(slint::TimerMode::Repeated, Duration::from_millis(200), {
        let (win, settings, weak) = (others.settings.clone(), settings.clone(), app.as_weak());
        move || {
            mac_window::appearance_all(settings.borrow().dark);
            // the main window never grows past the screen (minus its title bar); longer pages scroll
            let h = mac_window::screen_height();
            if let Some(app) = weak.upgrade() {
                if h > 0.0 {
                    app.set_screen_height((h - 28.0) as f32);
                }
                // tab bar and title bar are one surface, as in the settings window
                let size = app.window().size().to_logical(app.window().scale_factor());
                mac_window::unify_titlebar(app.window(), size.width as f64, size.height as f64);
            }
            if let Some(w) = win.borrow().as_ref() {
                // the NSWindow exists only once the event loop has run after show(); only changes it once
                mac_window::unify_titlebar(w.window(), 800.0, 450.0);
            }
        }
    });
    #[cfg(target_os = "macos")]
    mac_window::on_key_change({
        let (win, weak) = (others.settings.clone(), app.as_weak());
        Box::new(move || {
            // called from inside AppKit (also while show() holds the window's RefCell): update on the next turn
            let (win, weak) = (win.clone(), weak.clone());
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(app) = weak.upgrade() {
                    let key = mac_window::is_key(app.window());
                    if app.get_active() != key {
                        app.set_active(key);
                    }
                }
                if let Some(w) = win.borrow().as_ref() {
                    let key = mac_window::is_key(w.window());
                    if w.get_active() != key {
                        w.set_active(key);
                    }
                }
            });
        })
    });

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
            // every picture the manual names exists
            for line in l.2.lines().filter(|l| l.starts_with("![")) {
                let path = line.split_once("](").unwrap().1.trim_end_matches(')');
                assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/help").join(path).is_file(), "{}: {path}", l.0);
            }
        }
        let (_, blocks) = super::help_blocks("## A\n\ntext\n\n![x](nowhere.png)\n\nmore");
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[2].picture.size().width, 0); // missing file: empty picture, not a crash
    }
}
