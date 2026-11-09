//! Self-test reports and problem reports (same JSON as Mountpoint's Reporter.swift). They go to the report
//! receiver (Cloudflare Worker in the Mountpoint repo, `worker/`), which files them as issues in the private repo
//! moritz-poth/mediafiles-check-reports. No key in the app: the worker validates and rate-limits by itself.
//! If sending fails, the report is saved in Downloads – nothing is lost. Everything is anonymized first.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const ENDPOINT: &str = "https://mountpoint-reports.moritz-poth.workers.dev/report";
/// tells the worker which repo the report belongs to
pub const PRODUCT: &str = "mediafiles-check";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CheckResult {
    pub id: String,
    pub title: String,
    /// "pass" | "fail" | "skip"
    pub status: &'static str,
    pub detail: String,
    pub seconds: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// "selftest" | "problem"
    pub kind: &'static str,
    pub product: &'static str,
    pub app: String,
    pub build: String,
    pub date: String,
    pub system: BTreeMap<String, String>,
    pub results: Vec<CheckResult>,
    pub duration_seconds: f64,
    pub log: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Sent,
    /// not sent; saved in Downloads at this path ("" = could not be saved either)
    Failed(String),
}

pub fn new(kind: &'static str, system: BTreeMap<String, String>, log: Vec<String>) -> Report {
    Report {
        kind,
        product: PRODUCT,
        app: env!("CARGO_PKG_VERSION").into(),
        build: env!("CARGO_PKG_VERSION").chars().take(16).collect(),
        date: chrono::Local::now().to_rfc3339(),
        system,
        results: vec![],
        duration_seconds: 0.0,
        log,
        note: None,
        contact: None,
    }
}

/// Problem report: description, optional contact, optional system info and log (anonymized).
pub fn problem(text: &str, contact: &str, attach: bool, lang: &str) -> Report {
    let mut r = new("problem", if attach { system_info(lang) } else { BTreeMap::new() }, if attach { log_tail(150) } else { vec![] });
    r.note = Some(text.trim().into());
    r.contact = (!contact.trim().is_empty()).then(|| contact.trim().into());
    r
}

/// Sends the report; if that fails, saves it in Downloads. Runs up to 30 s – call it off the UI thread.
pub fn send(report: &Report) -> Outcome {
    let body = serde_json::to_vec(report).unwrap_or_default();
    let tmp = std::env::temp_dir().join(format!("mfc-report-{}.json", std::process::id()));
    let sent = std::fs::write(&tmp, &body).is_ok()
        && Command::new("/usr/bin/curl")
            .args(["-fsS", "--max-time", "30", "-X", "POST", "-H", "Content-Type: application/json"])
            .args(["-A", &format!("MediaFilesCheck/{}", report.app), "--data-binary"])
            .arg(format!("@{}", tmp.display()))
            .arg(ENDPOINT)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&tmp);
    if sent {
        crate::log::write(&format!("report sent: {}", report.kind));
        return Outcome::Sent;
    }
    let saved = save(report).map(|p| p.display().to_string()).unwrap_or_default();
    crate::log::write(&format!("report not sent, saved: {saved}"));
    Outcome::Failed(saved)
}

/// Saves the report as JSON in Downloads.
pub fn save(report: &Report) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("Downloads");
    let path = dir.join(format!("MediaFiles-Check-{}-{}.json", report.kind, chrono::Local::now().format("%Y-%m-%d-%H%M%S")));
    std::fs::write(&path, serde_json::to_vec_pretty(report).ok()?).ok()?;
    Some(path)
}

fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn plist_version(app: &str) -> Option<String> {
    let v = output("/usr/bin/plutil", &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", &format!("{app}/Contents/Info.plist")]);
    (!v.is_empty()).then_some(v)
}

/// System, hardware and Media Composer – nothing personal (no names, no paths).
pub fn system_info(lang: &str) -> BTreeMap<String, String> {
    let mut s = BTreeMap::new();
    s.insert("macOS".into(), format!("{} ({})", output("/usr/bin/sw_vers", &["-productVersion"]), output("/usr/bin/sw_vers", &["-buildVersion"])));
    s.insert("model".into(), output("/usr/sbin/sysctl", &["-n", "hw.model"]));
    s.insert("cpu".into(), output("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]));
    s.insert("arch".into(), std::env::consts::ARCH.replace("aarch64", "arm64"));
    let mem: u64 = output("/usr/sbin/sysctl", &["-n", "hw.memsize"]).parse().unwrap_or(0);
    s.insert("memoryGB".into(), (mem >> 30).to_string());
    s.insert("language".into(), output("/usr/bin/defaults", &["read", "-g", "AppleLocale"]));
    s.insert("appLanguage".into(), lang.into());
    let mc: Vec<String> = std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("Avid Media Composer"))
        .map(|e| {
            let dir = e.path();
            let items: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|i| i.file_name().to_string_lossy().into_owned()).collect();
            let app = items.iter().find(|i| i.ends_with(".app") && i.replace(' ', "").contains("MediaComposer"));
            let v = app.and_then(|a| plist_version(&dir.join(a).to_string_lossy())).unwrap_or_else(|| "installed".into());
            v + if items.iter().any(|i| i == "Media Composer Enterprise") { " (Enterprise folder present)" } else { "" }
        })
        .collect();
    s.insert("mediaComposer".into(), if mc.is_empty() { "not installed".into() } else { mc.join(", ") });
    s.insert("drivesWithMediaFiles".into(), crate::find_volumes().len().to_string());
    s
}

/// Last lines of the log file, anonymized.
pub fn log_tail(lines: usize) -> Vec<String> {
    let text = std::fs::read_to_string(crate::log::path()).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].iter().map(|l| anonymize(l)).collect()
}

fn hash_id(s: &str) -> String {
    Sha256::digest(s.as_bytes())[..3].iter().map(|b| format!("{b:02x}")).collect()
}

/// Replaces the user name, the computer name, server addresses and every folder or drive name in paths
/// (`/Volumes/…`, `/Users/…`, `~/…`) with short hash ids – the same name always gets the same id, so a report
/// still shows which paths are the same.
pub fn anonymize(text: &str) -> String {
    let mut t = text.to_string();
    if let Some(home) = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned()).filter(|h| h.len() > 1) {
        t = t.replace(&home, "~");
    }
    let user = output("/usr/bin/id", &["-un"]);
    if user.chars().count() > 2 {
        t = t.replace(&user, "<user>");
    }
    let computer = crate::rename::computer_name();
    if computer.chars().count() > 2 {
        t = t.replace(&computer, "<computer>");
    }
    // server addresses (smb://user@nas/share): scheme stays, the rest becomes an id
    let mut from = 0;
    while let Some(i) = t[from..].find("://").map(|i| from + i) {
        let start = t[..i].rfind(|c: char| !c.is_ascii_alphabetic()).map_or(0, |s| s + 1);
        let scheme = t[start..i].to_ascii_lowercase();
        if !["smb", "afp", "nfs", "cifs"].contains(&scheme.as_str()) {
            from = i + 3;
            continue;
        }
        let end = t[i + 3..].find(|c: char| c.is_whitespace() || "),\"'".contains(c)).map_or(t.len(), |e| i + 3 + e);
        let replacement = format!("{scheme}://<server-{}>", hash_id(&t[start..end]));
        t.replace_range(start..end, &replacement);
        from = start + replacement.len();
    }
    const KEEP: [&str; 12] = ["Avid MediaFiles", "MXF", "1", "Library", "Logs", "MediaFiles Check", "Application Support", "Volumes", "Users", "Shared", "Downloads", "Avid Projects"];
    let mut out = String::new();
    let mut rest = t.as_str();
    loop {
        let next = ["/Volumes/", "/Users/", "~/"].iter().filter_map(|p| rest.find(p).map(|i| (i, *p))).min();
        let Some((i, prefix)) = next else { break };
        out.push_str(&rest[..i + prefix.len()]);
        rest = &rest[i + prefix.len()..];
        // the path ends at a separator or at " → " / " – " (log arrows and dashes); single spaces inside names stay
        let mut end = rest.len();
        for (j, c) in rest.char_indices() {
            if "\t\n,:\"'|()".contains(c) || rest[j..].starts_with(" → ") || rest[j..].starts_with(" – ") || rest[j..].starts_with("  ") {
                end = j;
                break;
            }
        }
        let path = &rest[..end];
        let parts: Vec<String> = path
            .split('/')
            .map(|p| if p.is_empty() || KEEP.contains(&p) || p.starts_with('<') || p.starts_with("MFCSelftest") { p.to_string() } else { format!("n{}", hash_id(p)) })
            .collect();
        out.push_str(&parts.join("/"));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymize_hides_names_but_keeps_structure() {
        let a = anonymize("renamed /Volumes/Client Drive/Avid MediaFiles/MXF/Avid10.1 → 2, smb://user@nas/share ok");
        assert!(a.starts_with("renamed /Volumes/n"), "{a}");
        assert!(a.contains("/Avid MediaFiles/MXF/n") && a.contains(" → 2,"), "{a}");
        assert!(!a.contains("Client") && !a.contains("Avid10.1") && !a.contains("nas"), "{a}");
        assert!(a.contains("smb://<server-"), "{a}");
        // the same name always gets the same id
        assert_eq!(anonymize("/Volumes/X/a"), anonymize("/Volumes/X/a"));
        assert_ne!(anonymize("/Volumes/X/a"), anonymize("/Volumes/Y/a"));
    }

    #[test]
    fn report_json_has_the_fields_the_worker_expects() {
        let mut r = problem("It broke", "", false, "de");
        r.results.push(CheckResult { id: "x".into(), title: "X".into(), status: "pass", detail: "ok".into(), seconds: 0.1 });
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        for key in ["kind", "product", "app", "build", "date", "system", "results", "durationSeconds", "log", "note"] {
            assert!(v.get(key).is_some(), "{key} missing");
        }
        assert!(v.get("contact").is_none());
        assert_eq!(v["product"], "mediafiles-check");
    }
}
