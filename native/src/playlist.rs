//! Playlist model + resume-position persistence, shared in spirit with the
//! web prototype's `src/playlist.ts`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Containers libmpv/ffmpeg handles. Unlike the web build, this list has no
/// "codec-dependent" tier — if ffmpeg can demux it, we play it.
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "vob",
    "ogv", "ogm", "rm", "rmvb", "asf", "3gp", "3g2", "divx", "f4v", "mxf", "y4m",
];

pub const SUB_EXTS: &[&str] = &["srt", "ass", "ssa", "sub", "vtt", "idx", "sup"];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn is_subtitle(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUB_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
}

impl Entry {
    pub fn new(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Entry { path, name }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

#[derive(Default)]
pub struct Playlist {
    pub items: Vec<Entry>,
    pub current: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Playlist {
    /// Add paths; directories are walked one level deep for video files.
    pub fn add_paths(&mut self, paths: &[PathBuf]) -> usize {
        let mut added = Vec::new();
        for p in paths {
            if p.is_dir() {
                if let Ok(rd) = std::fs::read_dir(p) {
                    let mut batch: Vec<PathBuf> = rd
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| is_video(p))
                        .collect();
                    batch.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
                    added.extend(batch);
                }
            } else if is_video(p) {
                added.push(p.clone());
            }
        }
        let n = added.len();
        self.items.extend(added.into_iter().map(Entry::new));
        n
    }

    pub fn current_entry(&self) -> Option<&Entry> {
        self.current.and_then(|i| self.items.get(i))
    }

    /// Index of the next track, honouring shuffle and repeat.
    /// `auto` distinguishes "track ended" from "user pressed next".
    pub fn next_index(&self, auto: bool) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let cur = self.current.unwrap_or(0);

        if auto && self.repeat == Repeat::One {
            return Some(cur);
        }

        if self.shuffle && self.items.len() > 1 {
            // Cheap xorshift off the clock; no rand dependency needed.
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(1);
            let mut n = seed % self.items.len();
            if n == cur {
                n = (n + 1) % self.items.len();
            }
            return Some(n);
        }

        let last = cur + 1 >= self.items.len();
        if last {
            if auto && self.repeat == Repeat::Off {
                return None; // stop at end of playlist
            }
            return Some(0);
        }
        Some(cur + 1)
    }

    pub fn prev_index(&self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let cur = self.current.unwrap_or(0);
        Some(if cur == 0 { self.items.len() - 1 } else { cur - 1 })
    }
}

/// "ep2" before "ep10".
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let mut nx = 0u64;
                    let mut ny = 0u64;
                    while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                        nx = nx.saturating_mul(10).saturating_add(c as u64 - '0' as u64);
                        ai.next();
                    }
                    while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                        ny = ny.saturating_mul(10).saturating_add(c as u64 - '0' as u64);
                        bi.next();
                    }
                    match nx.cmp(&ny) {
                        std::cmp::Ordering::Equal => continue,
                        o => return o,
                    }
                } else {
                    let (lx, ly) = (x.to_ascii_lowercase(), y.to_ascii_lowercase());
                    match lx.cmp(&ly) {
                        std::cmp::Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        o => return o,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Persisted state: resume positions + preferences
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    /// path -> seconds
    pub resume: HashMap<String, f64>,
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub shuffle: bool,
    pub repeat: String,
}

impl State {
    fn path() -> Option<PathBuf> {
        let dir = dirs::config_dir()?.join("LarrezPlayer");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join("state.json"))
    }

    pub fn load() -> Self {
        let mut s: State = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if s.volume <= 0.0 {
            s.volume = 100.0;
        }
        if s.speed <= 0.0 {
            s.speed = 1.0;
        }
        s
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Ok(t) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, t);
            }
        }
    }

    /// Remember a position, ignoring trivial starts and near-complete playback.
    pub fn remember(&mut self, path: &Path, pos: f64, dur: f64) {
        let key = path.to_string_lossy().into_owned();
        if dur > 0.0 && pos > 15.0 && pos < dur - 20.0 {
            self.resume.insert(key, pos);
        } else {
            self.resume.remove(&key);
        }
        // Bound the file so it can't grow forever.
        if self.resume.len() > 300 {
            if let Some(k) = self.resume.keys().next().cloned() {
                self.resume.remove(&k);
            }
        }
    }

    pub fn resume_for(&self, path: &Path) -> Option<f64> {
        self.resume.get(&path.to_string_lossy().into_owned()).copied()
    }
}

pub fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let t = secs as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_orders_episodes() {
        let mut v = vec!["ep10.mkv", "ep2.mkv", "ep1.mkv"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["ep1.mkv", "ep2.mkv", "ep10.mkv"]);
    }

    #[test]
    fn recognises_containers() {
        assert!(is_video(Path::new("a.mkv")));
        assert!(is_video(Path::new("A.AVI")));
        assert!(!is_video(Path::new("a.txt")));
        assert!(is_subtitle(Path::new("a.srt")));
    }

    #[test]
    fn time_formatting() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(75.0), "1:15");
        assert_eq!(fmt_time(3725.0), "1:02:05");
    }

    #[test]
    fn repeat_off_stops_at_end() {
        let mut pl = Playlist::default();
        pl.items = vec![Entry::new("a.mkv".into()), Entry::new("b.mkv".into())];
        pl.current = Some(1);
        assert_eq!(pl.next_index(true), None);
        pl.repeat = Repeat::All;
        assert_eq!(pl.next_index(true), Some(0));
    }

    #[test]
    fn resume_ignores_trivial_and_final_positions() {
        let mut s = State::default();
        let p = Path::new("x.mkv");
        s.remember(p, 5.0, 600.0);
        assert_eq!(s.resume_for(p), None);
        s.remember(p, 300.0, 600.0);
        assert_eq!(s.resume_for(p), Some(300.0));
        s.remember(p, 595.0, 600.0);
        assert_eq!(s.resume_for(p), None);
    }
}
