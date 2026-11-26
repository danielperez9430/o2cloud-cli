//! The run log: what a command did, for finding out afterwards why
//! something went wrong (see `debug-report`).
//!
//! Every run writes one JSON-lines file to `<cache dir>/o2cli/logs`
//! (the last `KEEP_RUNS` are kept): at the `info` level by default, at
//! `debug` with `--debug` or `O2CLOUD_LOG=debug`. On stderr nothing
//! more is shown unless asked for (`--debug`, `O2CLOUD_LOG`), and then
//! each line starts with `[debug]`, `[info]`… stdout is never written
//! to: it belongs to the command's result (`--json`).
//!
//! Everything goes through `redact::text` here, so no call can leak a
//! session key. Names of files and folders go in `fields` (`name`,
//! `path`, `local`, `cloud`), never in `msg`, so `debug-report` can
//! replace them.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// How many runs' logs are kept.
const KEEP_RUNS: usize = 10;
/// How many crash reports are kept.
const KEEP_CRASHES: usize = 5;
/// The most a run's log takes: past it, only warnings, errors and the
/// end of the run are written.
const CAP_BYTES: u64 = 5 * 1024 * 1024;
const CAP_BYTES_DEBUG: u64 = 50 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }

    fn parse(s: &str) -> Option<Option<Level>> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => None,
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" | "1" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => return None,
        })
    }
}

struct FileSink {
    writer: std::io::BufWriter<std::fs::File>,
    written: u64,
    cap: u64,
    /// Whether the cap was reached (and said so in the log).
    capped: bool,
    /// When it was last written to disk.
    flushed: std::time::Instant,
}

/// How often what's buffered goes to disk, so a run cut short (Ctrl-C, a
/// closed window) still leaves its log.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
/// A log written to this recently may be a run still going: never pruned.
const IN_USE: std::time::Duration = std::time::Duration::from_secs(3600);

struct Logger {
    run: String,
    /// The most detailed level shown on stderr (0: none).
    stderr: AtomicU64,
    /// The most detailed level written to the file.
    file_level: Level,
    file: Option<Mutex<FileSink>>,
    path: Option<PathBuf>,
    start: std::time::Instant,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// `<cache dir>/o2cli/logs`, where run logs and crash reports go.
pub fn dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("o2cli").join("logs"))
}

/// Start logging. `debug` is `--debug`: debug detail on stderr and in the
/// file. `O2CLOUD_LOG` sets the stderr level (`off`, `error`, `warn`,
/// `info`, `debug`, `trace`), and `O2CLOUD_LOG_FILE` the file (`0` for
/// none, or a path).
pub fn init(debug: bool, to_file: bool) {
    let env_level = std::env::var("O2CLOUD_LOG")
        .ok()
        .and_then(|v| Level::parse(&v));
    let stderr = match (debug, env_level) {
        (true, _) => Some(Level::Debug),
        (false, Some(level)) => level,
        (false, None) => None,
    };
    let detailed = stderr.is_some_and(|l| l >= Level::Debug);
    let file_level = if detailed { Level::Debug } else { Level::Info };
    let run = run_id_new();
    let opened = if to_file || detailed {
        open_file(&run)
    } else {
        None
    };
    let (file, path) = match opened {
        Some((f, p)) => (
            Some(Mutex::new(FileSink {
                writer: std::io::BufWriter::new(f),
                written: 0,
                cap: if detailed { CAP_BYTES_DEBUG } else { CAP_BYTES },
                capped: false,
                flushed: std::time::Instant::now(),
            })),
            Some(p),
        ),
        None => (None, None),
    };
    let _ = LOGGER.set(Logger {
        run,
        stderr: AtomicU64::new(stderr.map_or(0, |l| l as u64)),
        file_level,
        file,
        path,
        start: std::time::Instant::now(),
    });
}

/// An 8-hex-digit ID for this run, to find its log from an error message.
fn run_id_new() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    format!("{:08x}", h.finish() as u32)
}

/// Open this run's log file, and remove the oldest ones.
fn open_file(run: &str) -> Option<(std::fs::File, PathBuf)> {
    let path = match std::env::var("O2CLOUD_LOG_FILE") {
        Ok(v) if v == "0" || v.is_empty() => return None,
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            let dir = dir()?;
            std::fs::create_dir_all(&dir).ok()?;
            prune(&dir, "run-", KEEP_RUNS - 1);
            let stamp = crate::util::system_time_to_compact_iso(std::time::SystemTime::now());
            dir.join(format!("run-{}-{}.jsonl", stamp, run))
        }
    };
    let file = crate::config::open_private_append(&path).ok()?;
    Some((file, path))
}

/// Keep only the newest `keep` files in `dir` whose names start with
/// `prefix` (their names sort by date).
fn prune(dir: &std::path::Path, prefix: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .collect();
    files.sort();
    let extra = files.len().saturating_sub(keep);
    for old in files.into_iter().take(extra) {
        // A long upload may still be writing to it.
        let in_use = std::fs::metadata(&old)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < IN_USE);
        if !in_use {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// This run's ID ("" before `init`).
pub fn run_id() -> &'static str {
    LOGGER.get().map_or("", |l| l.run.as_str())
}

/// This run's log file, if it has one.
pub fn path() -> Option<&'static PathBuf> {
    LOGGER.get().and_then(|l| l.path.as_ref())
}

/// Whether something at `level` would be written anywhere, to skip
/// building costly records.
pub fn enabled(level: Level) -> bool {
    let Some(l) = LOGGER.get() else {
        return false;
    };
    level as u64 <= l.stderr.load(Ordering::Relaxed) || (l.file.is_some() && level <= l.file_level)
}

/// Log `msg` at `level` from `target` (`http`, `auth`, `cache`,
/// `decide`…), with `fields` (a JSON object, or null).
pub fn log(level: Level, target: &str, msg: &str, fields: serde_json::Value) {
    let Some(l) = LOGGER.get() else {
        return;
    };
    let to_stderr = level as u64 <= l.stderr.load(Ordering::Relaxed);
    let to_file = l.file.is_some() && level <= l.file_level;
    if !to_stderr && !to_file {
        return;
    }
    let msg = crate::redact::text(msg);
    let fields = redact_value(fields);
    if to_stderr {
        let mut line = format!("[{}] {}: {}", level.name(), target, msg);
        if let serde_json::Value::Object(map) = &fields {
            for (k, v) in map {
                match v {
                    serde_json::Value::String(s) => line.push_str(&format!(" {}={:?}", k, s)),
                    v => line.push_str(&format!(" {}={}", k, v)),
                }
            }
        }
        crate::ui::line(&line);
    }
    if let (true, Some(file)) = (to_file, &l.file) {
        let record = serde_json::json!({
            "ts": crate::util::system_time_to_compact_iso(std::time::SystemTime::now()),
            "ms": l.start.elapsed().as_millis() as u64,
            "lvl": level.name(),
            "run": l.run,
            "target": target,
            "msg": msg,
            "fields": fields,
        });
        let mut sink = file.lock().unwrap_or_else(|e| e.into_inner());
        // Past the cap, only what matters to find out what went wrong.
        if sink.written > sink.cap && level > Level::Warn && target != "run" {
            if !sink.capped {
                sink.capped = true;
                let note = serde_json::json!({
                    "lvl": "warn", "run": l.run, "target": "log",
                    "msg": "log full: only warnings, errors and the end from here",
                });
                let _ = writeln!(sink.writer, "{}", note);
            }
            return;
        }
        let line = format!("{}\n", record);
        if sink.writer.write_all(line.as_bytes()).is_ok() {
            sink.written += line.len() as u64;
        }
        if level <= Level::Warn || sink.flushed.elapsed() >= FLUSH_EVERY {
            let _ = sink.writer.flush();
            sink.flushed = std::time::Instant::now();
        }
    }
}

/// Write what's buffered to the file (at exit, and before a crash report).
pub fn flush() {
    if let Some(file) = LOGGER.get().and_then(|l| l.file.as_ref()) {
        let _ = file
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .writer
            .flush();
    }
}

fn redact_value(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(crate::redact::text(&s)),
        serde_json::Value::Array(a) => a.into_iter().map(redact_value).collect(),
        serde_json::Value::Object(m) => m.into_iter().map(|(k, v)| (k, redact_value(v))).collect(),
        v => v,
    }
}

pub fn error(target: &str, msg: &str, fields: serde_json::Value) {
    log(Level::Error, target, msg, fields)
}
pub fn warn(target: &str, msg: &str, fields: serde_json::Value) {
    log(Level::Warn, target, msg, fields)
}
pub fn info(target: &str, msg: &str, fields: serde_json::Value) {
    log(Level::Info, target, msg, fields)
}
pub fn debug(target: &str, msg: &str, fields: serde_json::Value) {
    log(Level::Debug, target, msg, fields)
}

/// Write a crash report for a panic (`info`) next to the run logs, and
/// return its path.
pub fn crash_report(info: &std::panic::PanicHookInfo<'_>) -> Option<PathBuf> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    prune(&dir, "crash-", KEEP_CRASHES - 1);
    let stamp = crate::util::system_time_to_compact_iso(std::time::SystemTime::now());
    let path = dir.join(format!("crash-{}-{}.txt", stamp, run_id()));
    let args: Vec<String> = std::env::args().collect();
    let report = format!(
        "o2cloud {} ({} {})\nrun {}\nargs: {}\n\n{}\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        run_id(),
        crate::redact::args(&args).join(" "),
        crate::redact::text(&info.to_string()),
        std::backtrace::Backtrace::force_capture()
    );
    let mut file = crate::config::open_private_append(&path).ok()?;
    file.write_all(report.as_bytes()).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levels() {
        assert_eq!(Level::parse("DEBUG"), Some(Some(Level::Debug)));
        assert_eq!(Level::parse("off"), Some(None));
        assert_eq!(Level::parse("loud"), None);
        assert!(Level::Error < Level::Debug);
    }

    #[test]
    fn test_prune() {
        let dir = std::env::temp_dir().join(format!("o2cli_prune_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        for i in 0..5 {
            let path = dir.join(format!("run-2026010{}.jsonl", i));
            std::fs::write(&path, b"x").unwrap();
            // The first is still being written to: kept.
            if i > 0 {
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
            }
        }
        std::fs::write(dir.join("crash-1.txt"), b"x").unwrap();
        prune(&dir, "run-", 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "crash-1.txt",
                "run-20260100.jsonl",
                "run-20260103.jsonl",
                "run-20260104.jsonl"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
