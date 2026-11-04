//! Log file for error analysis: ~/Library/Logs/MediaFiles Check/MediaFiles Check.log (Windows: %APPDATA%\MediaFiles Check).
//! One line per event with local time; at 1 MB the file moves to "… .old.log" and a new one starts.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 1_000_000;

pub fn path() -> PathBuf {
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("MediaFiles Check"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Logs/MediaFiles Check"))
    };
    dir.unwrap_or_else(|| PathBuf::from(".")).join("MediaFiles Check.log")
}

/// Appends a line. Logging never stops the app: errors while writing are ignored.
pub fn write(msg: &str) {
    let p = path();
    let _ = fs::create_dir_all(p.parent().unwrap_or(&p));
    if fs::metadata(&p).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(&p, p.with_extension("old.log"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg.replace('\n', " / "));
    }
}
