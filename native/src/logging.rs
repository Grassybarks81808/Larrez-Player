//! A very small logger, so a bad day leaves evidence behind.
//!
//! Everything (our own messages plus mpv's) is appended to
//! `%APPDATA%\LarrezPlayer\larrez.log`. That file is the difference between
//! "the window is blank, no idea why" and a bug report we can act on: mpv is
//! the only component that knows *why* a video output refused to start, and by
//! default it prints that to a console the user does not have.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Roll the log over rather than letting it grow forever.
const MAX_BYTES: u64 = 512 * 1024;
const FILE_NAME: &str = "larrez.log";

struct Sink {
    file: Option<std::fs::File>,
    verbose: bool,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Where the log lives, for telling users where to look.
pub fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("LarrezPlayer").join(FILE_NAME))
}

pub fn path_display() -> String {
    path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no config directory available)".into())
}

/// Open the log for this run and record that we started.
pub fn init(verbose: bool) {
    let sink = Sink {
        file: open_log_file(),
        verbose,
    };
    let _ = SINK.set(Mutex::new(sink));
    log(
        "info",
        &format!(
            "Larrez Player {} starting on {} {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
    if let Some(p) = path() {
        log("info", &format!("log file: {}", p.display()));
    }
}

fn open_log_file() -> Option<std::fs::File> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    if std::fs::metadata(&p)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&p);
    }
    OpenOptions::new().create(true).append(true).open(&p).ok()
}

/// Write one line. Safe to call before `init` (and after a panic).
pub fn log(level: &str, message: &str) {
    let line = format_line(level, message);
    let mirror = should_mirror(level);

    match SINK.get() {
        Some(sink) => {
            let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(f) = sink.file.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            if mirror || sink.verbose {
                mirror_to_stderr(&line);
            }
        }
        None => {
            // Not initialised yet: stderr is all we have.
            mirror_to_stderr(&line);
        }
    }
}

/// Log a message that came from mpv (its text already carries a newline).
pub fn log_mpv(level: &str, prefix: &str, text: &str) {
    let text = text.trim_end_matches(['\r', '\n']);
    log(level, &format!("{prefix}{text}"));
}

/// A GUI process usually has no console at all, and `eprintln!` panics when the
/// write fails — so write and ignore the result instead.
fn mirror_to_stderr(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// `[level] message`, padded so the levels line up when reading the file.
fn format_line(level: &str, message: &str) -> String {
    format!("[{level:<5}] {message}")
}

/// Problems are worth showing on stderr as well as in the file.
fn should_mirror(level: &str) -> bool {
    matches!(level, "warn" | "error" | "fatal")
}

/// Replace the default panic hook: log the panic, then tell the user — a GUI
/// process has no console to show it in.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log("fatal", &format!("panic: {info}"));
        log("fatal", &format!("details in {}", path_display()));
        previous(info);

        #[cfg(windows)]
        crate::win::message_box(
            "Larrez Player hit an internal error",
            &format!("{info}\n\nThe details were written to:\n{}", path_display()),
        );
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_padded_for_readability() {
        assert_eq!(format_line("info", "hello"), "[info ] hello");
        assert_eq!(format_line("error", "boom"), "[error] boom");
    }

    #[test]
    fn only_problems_mirror_to_stderr() {
        assert!(should_mirror("warn"));
        assert!(should_mirror("error"));
        assert!(should_mirror("fatal"));
        assert!(!should_mirror("info"));
    }

    #[test]
    fn mpv_text_does_not_double_up_newlines() {
        // log_mpv trims; the formatting itself is trivial but the trim is not.
        assert_eq!("hello\n".trim_end_matches(['\r', '\n']), "hello");
    }
}
