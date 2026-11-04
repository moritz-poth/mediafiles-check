//! Command line for the core (scripting and testing); the app is src/app.rs.
use mediafiles_check::{check, find_volumes, report_text};
use std::path::PathBuf;
use std::process::ExitCode;

/// mfcheck [--de] [DRIVE or FOLDER...]   (default: all drives with Avid MediaFiles/MXF)
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--projects") {
        return projects(&args[1..]);
    }
    let lang = if args.iter().any(|a| a == "--de") { "de" } else { "en" };
    let mut roots: Vec<PathBuf> = args.iter().filter(|a| *a != "--de").map(PathBuf::from).collect();
    if roots.is_empty() {
        roots = find_volumes();
    }
    let reports = check(&roots, &mut |i, n, p| eprint!("\r{}/{n} {p}\x1b[K", i + 1));
    eprintln!();
    print!("{}", report_text(&reports, lang));
    if reports.iter().any(|r| !r.findings.is_empty()) { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// mfcheck --projects FOLDER [REQUEST.ferry]: media missing for the Avid projects below FOLDER, checked against the
/// connected drives (or the folders in MFC_DRIVES, colon-separated); optionally writes the request for Ferry.
fn projects(args: &[String]) -> ExitCode {
    use mediafiles_check::projects;
    let Some(root) = args.first() else {
        eprintln!("mfcheck --projects FOLDER [REQUEST.ferry]");
        return ExitCode::FAILURE;
    };
    let drives: Vec<PathBuf> = match std::env::var("MFC_DRIVES") {
        Ok(d) => d.split(':').map(PathBuf::from).collect(),
        Err(_) => find_volumes(),
    };
    let a = projects::available_media(&drives, std::time::Duration::from_secs(30));
    eprintln!("{} media files on {} drives ({} MXF files in no database identified by their header, {} unidentified, skipped: {:?})", a.ids.len(), drives.len(), a.from_header, a.unknown, a.skipped);
    let available = a.ids;
    let found = projects::find_projects(root.as_ref());
    let r = projects::scan(&found, &available, &mut |i, n, b| eprint!("\r{}/{n} {}\x1b[K", i + 1, b.chars().take(60).collect::<String>()));
    eprintln!();
    print!("{}", projects::list_text(&r, "en"));
    if let Some(out) = args.get(1) {
        let system = mediafiles_check::rename::computer_name();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Err(e) = std::fs::write(out, projects::request_json(&system, now, &r.missing)) {
            eprintln!("{out}: {e}");
            return ExitCode::FAILURE;
        }
        eprintln!("request: {out}");
    }
    ExitCode::SUCCESS
}
