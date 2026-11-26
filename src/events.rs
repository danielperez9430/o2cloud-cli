//! `--json-lines`: what a transfer does, as it happens, for apps. One
//! JSON object per line on stdout, each with `event`:
//!
//! - `start`: `command`, `files`, `bytes`, `schema` (this format's
//!   version), `run`.
//! - `progress`: a file's `local` (or `name`), `bytes` sent or received so
//!   far and its `size`; at most every `PROGRESS_EVERY` per file.
//! - `file`: a file done, as its `--json` entry (`action`, `ids`…) plus,
//!   for an upload sent whole in this run, its `sha256`.
//! - `retry`: the server was busy or failing and the request goes again.
//! - `summary`: the counts at the end.
//! - `error`: the error a run ended with, as in `--json` (then the exit
//!   code says what kind).
//!
//! stdout carries nothing else then; messages stay on stderr.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// This format's version, in `start`; it changes only if a field changes
/// meaning or goes.
pub const SCHEMA: u64 = 1;
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

static ON: AtomicBool = AtomicBool::new(false);
static OUT: Mutex<()> = Mutex::new(());

pub fn enable() {
    ON.store(true, Ordering::Relaxed);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Print `fields` as an `event` line (only with `--json-lines`).
pub fn emit(event: &str, fields: serde_json::Value) {
    if !on() {
        return;
    }
    let mut record = serde_json::Map::new();
    record.insert("event".into(), event.into());
    if let serde_json::Value::Object(map) = fields {
        record.extend(map);
    }
    let _guard = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::Value::Object(record));
    let _ = out.flush();
}

/// The file a transfer future is working on, for `progress` events and
/// its SHA-256 (see `track`).
pub struct FileProgress {
    /// `{"local": …}` or `{"name": …}`: how `progress` names it.
    key: serde_json::Value,
    size: AtomicU64,
    moved: AtomicU64,
    last: Mutex<Instant>,
    /// The SHA-256 of what was read to be sent, while it is all of the
    /// file in order; `None` once a part wasn't sent (already there).
    hash: Mutex<Option<(Sha256, u64)>>,
}

tokio::task_local! {
    static FILE: Arc<FileProgress>;
}

/// Run `work` as the transfer of the file `key` names: the bytes the API
/// layer moves meanwhile are its progress, and what an upload reads is
/// hashed.
pub async fn track<F: std::future::Future>(
    key: serde_json::Value,
    size: u64,
    work: F,
) -> F::Output {
    if !on() {
        return work.await;
    }
    let file = Arc::new(FileProgress {
        key,
        size: AtomicU64::new(size),
        moved: AtomicU64::new(0),
        last: Mutex::new(Instant::now()),
        hash: Mutex::new(Some((Sha256::new(), 0))),
    });
    FILE.scope(file, work).await
}

fn with_file<T>(f: impl FnOnce(&FileProgress) -> T) -> Option<T> {
    FILE.try_with(|file| f(file)).ok()
}

/// The tracked file, to hand to code that runs outside this task: an
/// upload's body is read by the HTTP client's own task, where `moved` and
/// `hash` wouldn't find it.
pub fn current() -> Option<Arc<FileProgress>> {
    FILE.try_with(Arc::clone).ok()
}

impl FileProgress {
    /// `bytes` of this file read to be sent: hashed, and its progress.
    pub fn sent(&self, bytes: &[u8]) {
        if let Some((hasher, n)) = self.hash.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            hasher.update(bytes);
            *n += bytes.len() as u64;
        }
        self.progress(bytes.len() as u64);
    }

    fn progress(&self, n: u64) {
        let moved = self.moved.fetch_add(n, Ordering::Relaxed) + n;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < PROGRESS_EVERY {
            return;
        }
        *last = Instant::now();
        let mut event = self.key.clone();
        event["bytes"] = moved.into();
        event["size"] = self.size.load(Ordering::Relaxed).into();
        emit("progress", event);
    }
}

/// The size of the file being tracked, once known.
pub fn set_size(size: u64) {
    with_file(|file| file.size.store(size, Ordering::Relaxed));
}

/// `n` bytes of the tracked file moved: a `progress` event, if it's time.
pub fn moved(n: u64) {
    with_file(|file| file.progress(n));
}

/// Bytes of the tracked file read to be uploaded (in this task).
#[cfg(test)]
pub fn hash(bytes: &[u8]) {
    with_file(|file| {
        if let Some((hasher, n)) = file.hash.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            hasher.update(bytes);
            *n += bytes.len() as u64;
        }
    });
}

/// The hash so far, to go back to if a piece is sent again.
pub fn hash_snapshot() -> Option<(Sha256, u64)> {
    with_file(|file| file.hash.lock().unwrap_or_else(|e| e.into_inner()).clone()).flatten()
}

/// Back to `snapshot` before a piece is sent again.
pub fn hash_restore(snapshot: &Option<(Sha256, u64)>) {
    if snapshot.is_some() {
        with_file(|file| *file.hash.lock().unwrap_or_else(|e| e.into_inner()) = snapshot.clone());
    }
}

/// Part of the file wasn't sent (it was already there): no hash of it.
pub fn hash_gap() {
    with_file(|file| *file.hash.lock().unwrap_or_else(|e| e.into_inner()) = None);
}

/// The SHA-256 of the tracked file, if all `size` bytes were read to be
/// sent in this run.
pub fn sha256(size: u64) -> Option<String> {
    with_file(|file| {
        let hash = file.hash.lock().unwrap_or_else(|e| e.into_inner());
        match hash.as_ref() {
            Some((hasher, n)) if *n == size && size > 0 => Some(
                hasher
                    .clone()
                    .finalize()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect(),
            ),
            _ => None,
        }
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_track_hash() {
        enable();
        let sha = track(serde_json::json!({"name": "x"}), 6, async {
            hash(b"abc");
            let snap = hash_snapshot();
            hash(b"zzz"); // a piece sent, then turned away…
            hash_restore(&snap);
            hash(b"def"); // …and sent again
            sha256(6)
        })
        .await;
        // sha256("abcdef")
        assert_eq!(
            sha.as_deref(),
            Some("bef57ec7f53a6d40beb640a780a639c83bc29ac8a9816f1fc6c5c6dcd93c4721")
        );
        let gap = track(serde_json::json!({"name": "y"}), 3, async {
            hash_gap();
            hash(b"abc");
            sha256(3)
        })
        .await;
        assert_eq!(gap, None);
        // Outside a tracked file nothing happens.
        hash(b"abc");
        assert_eq!(sha256(3), None);
    }
}
