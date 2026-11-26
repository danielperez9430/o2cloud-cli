//! What the CLI tells the person running it, on stderr: how much (`-q`,
//! `-v`) and a progress bar for long transfers.
//!
//! - `detail!`: a line per file (uploaded, skipped, each part…), only with
//!   `-v`. Without it a large `upload-dir` would print hundreds of
//!   thousands of lines.
//! - `info!`: what a command did and its summary; not with `-q`.
//! - `warn!`: what needs attention (failures, conflicts, files left out);
//!   always.
//!
//! The progress bar is one line redrawn in place at a terminal; without
//! one (stderr in a log file) a status line every `PLAIN_EVERY`. It
//! counts the bytes the API layer moves (`add_bytes`) and the files the
//! command finishes (`file_done`). Lines printed while it's drawn clear
//! it first, and it's drawn again on the next tick.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::util::format_size;

/// 0: `-q`, 1: normal, 2: `-v`.
static LEVEL: AtomicU8 = AtomicU8::new(1);
/// Bytes sent or received for files so far (all transfers of the run).
static BYTES: AtomicU64 = AtomicU64::new(0);
/// Whether the bar is on the screen now (so a line must clear it first).
static DRAWN: AtomicBool = AtomicBool::new(false);
/// How many characters the bar took when last drawn, to clear it with
/// spaces: no ANSI codes, which older Windows consoles print as they are.
static DRAWN_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The bar's widest: one line on an 80-column terminal, so it never wraps
/// (a wrapped line can't be redrawn in place).
const BAR_WIDTH: usize = 70;

/// Blank the bar's line and go back to its start.
fn clear(err: &mut impl Write) {
    let len = DRAWN_LEN.load(Ordering::Relaxed);
    let _ = write!(err, "\r{}\r", " ".repeat(len));
}
/// Serialises writes to stderr between lines and the bar.
static OUT: Mutex<()> = Mutex::new(());

/// How often the bar is redrawn at a terminal.
const TICK: Duration = Duration::from_millis(250);
/// How often a status line is printed without a terminal.
const PLAIN_EVERY: Duration = Duration::from_secs(60);

pub fn set_level(quiet: bool, verbose: bool) {
    LEVEL.store(
        match (quiet, verbose) {
            (true, _) => 0,
            (false, true) => 2,
            (false, false) => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn level() -> u8 {
    LEVEL.load(Ordering::Relaxed)
}

/// Count `n` bytes of a file moved (called by the API layer as data goes).
pub fn add_bytes(n: u64) {
    count_bytes(n);
    crate::events::moved(n);
}

/// Count `n` bytes for the bar only (`--json-lines` progress is counted
/// by the caller).
pub fn count_bytes(n: u64) {
    BYTES.fetch_add(n, Ordering::Relaxed);
}

/// Print a line on stderr, clearing the bar first if it's drawn.
pub fn line(text: &str) {
    let _guard = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut err = std::io::stderr().lock();
    if DRAWN.swap(false, Ordering::Relaxed) {
        clear(&mut err);
    }
    let _ = writeln!(err, "{}", text);
}

#[macro_export]
macro_rules! detail {
    ($($arg:tt)*) => {
        if $crate::ui::level() >= 2 {
            $crate::ui::line(&format!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        if $crate::ui::level() >= 1 {
            $crate::ui::line(&format!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::ui::line(&format!($($arg)*))
    };
}

/// A running transfer's totals, for the bar.
struct Totals {
    label: &'static str,
    files: u64,
    bytes: u64,
    files_done: AtomicU64,
    /// Bytes of files finished without moving them (already there,
    /// skipped), so they don't count as still to go.
    bytes_skipped: AtomicU64,
    bytes_at_start: u64,
    start: Instant,
}

static CURRENT: Mutex<Option<Arc<Totals>>> = Mutex::new(None);

/// The bar of a transfer; it goes when this is dropped.
pub struct Bar {
    stop: Arc<AtomicBool>,
}

impl Drop for Bar {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _guard = OUT.lock().unwrap_or_else(|e| e.into_inner());
        if DRAWN.swap(false, Ordering::Relaxed) {
            clear(&mut std::io::stderr());
        }
    }
}

/// Start the bar for `files` files of `bytes` bytes in all (`label`:
/// "Uploading", "Downloading"). Nothing is shown with `-q`.
pub fn start(label: &'static str, files: u64, bytes: u64) -> Bar {
    use std::io::IsTerminal;
    let stop = Arc::new(AtomicBool::new(false));
    if files == 0 {
        return Bar { stop };
    }
    if level() == 0 {
        // No bar with -q, but the run's log is still marked in use.
        let stopped = stop.clone();
        tokio::spawn(async move {
            while !stopped.load(Ordering::Relaxed) {
                tokio::time::sleep(PLAIN_EVERY).await;
                if !stopped.load(Ordering::Relaxed) {
                    crate::log::heartbeat();
                }
            }
        });
        return Bar { stop };
    }
    let totals = Arc::new(Totals {
        label,
        files,
        bytes,
        files_done: AtomicU64::new(0),
        bytes_skipped: AtomicU64::new(0),
        bytes_at_start: BYTES.load(Ordering::Relaxed),
        start: Instant::now(),
    });
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(totals.clone());
    let terminal = std::io::stderr().is_terminal();
    let stopped = stop.clone();
    tokio::spawn(async move {
        let mut last_plain = Instant::now();
        let mut last_beat = Instant::now();
        loop {
            tokio::time::sleep(TICK).await;
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            if last_beat.elapsed() >= PLAIN_EVERY {
                last_beat = Instant::now();
                crate::log::heartbeat();
            }
            if terminal {
                let text = render(&totals, true);
                let _guard = OUT.lock().unwrap_or_else(|e| e.into_inner());
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                // Over the last one, padded with spaces where it was longer.
                let len = text.chars().count();
                let pad = DRAWN_LEN.load(Ordering::Relaxed).saturating_sub(len);
                let _ = write!(std::io::stderr(), "\r{}{}", text, " ".repeat(pad));
                let _ = std::io::stderr().flush();
                DRAWN_LEN.store(len.max(len + pad), Ordering::Relaxed);
                DRAWN.store(true, Ordering::Relaxed);
            } else if last_plain.elapsed() >= PLAIN_EVERY {
                last_plain = Instant::now();
                line(&render(&totals, false));
            }
        }
    });
    Bar { stop }
}

/// Count a file finished; `moved` false if its `size` bytes weren't
/// transferred (already there, skipped, failed).
pub fn file_done(size: u64, moved: bool) {
    if let Some(t) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        t.files_done.fetch_add(1, Ordering::Relaxed);
        if !moved {
            t.bytes_skipped.fetch_add(size, Ordering::Relaxed);
        }
    }
}

fn render(t: &Totals, short: bool) -> String {
    let moved = BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(t.bytes_at_start);
    let done = t.files_done.load(Ordering::Relaxed);
    let skipped = t.bytes_skipped.load(Ordering::Relaxed);
    let secs = t.start.elapsed().as_secs_f64().max(0.001);
    let speed = moved as f64 / secs;
    // A piece sent again counts twice; never past the total.
    let moved = moved.min(t.bytes.saturating_sub(skipped));
    let left = t.bytes.saturating_sub(moved + skipped);
    let eta = if speed > 1.0 && moved > 0 {
        format!(" · {} left", duration((left as f64 / speed) as u64))
    } else {
        String::new()
    };
    let text = format!(
        "{} {}/{} files · {} of {} · {}/s{}",
        t.label,
        done,
        t.files,
        format_size(moved + skipped),
        format_size(t.bytes),
        format_size(speed as u64),
        eta
    );
    if short {
        // One line on any terminal: no wrapping, which would break the
        // redraw.
        text.chars().take(BAR_WIDTH).collect()
    } else {
        format!("  … {}", text)
    }
}

/// `90` → `1 min 30 s`, `7200` → `2 h 0 min`.
fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{} s", s),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration() {
        assert_eq!(duration(5), "5 s");
        assert_eq!(duration(90), "1 min 30 s");
        assert_eq!(duration(7260), "2 h 1 min");
    }

    #[test]
    fn test_render() {
        let t = Totals {
            label: "Uploading",
            files: 10,
            bytes: 1000,
            files_done: AtomicU64::new(3),
            bytes_skipped: AtomicU64::new(200),
            bytes_at_start: BYTES.load(Ordering::Relaxed),
            start: Instant::now(),
        };
        let r = render(&t, false);
        assert!(
            r.starts_with("  … Uploading 3/10 files · 200 B of 1000 B"),
            "{r}"
        );
        assert!(render(&t, true).chars().count() <= BAR_WIDTH);
        assert!(!render(&t, true).contains('\x1b'));
    }
}
