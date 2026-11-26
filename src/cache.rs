//! A local copy of the account's file list, so commands that need every
//! file don't list the whole account each time (one request per 1,000
//! files). `O2Client::get_all_media` keeps it up to date with the
//! server's change log (`/sapi/profile/changes`), which reports every file
//! added, changed, moved or deleted since a point in time.
//!
//! The file holds file names, so it gets the same permissions as the
//! session file, and `logout` deletes it. `O2CLOUD_CACHE=0` turns it off.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Bumped when the format changes; an older file is ignored (2: files
/// carry `modificationdate`).
const VERSION: u32 = 2;

/// After this long the whole account is listed again, in case the change
/// log ever missed something.
pub const MAX_AGE_SECS: u64 = 7 * 24 * 3600;

/// The file list as of `cursor`.
#[derive(Serialize, Deserialize)]
pub struct MediaCache {
    version: u32,
    /// Whose files these are (the phone number of the session).
    pub account: String,
    /// Server time (Unix ms) up to which the change log has been applied.
    pub cursor: u64,
    /// When the whole account was last listed (Unix seconds).
    pub listed_at: u64,
    /// Files the change log reported but the server didn't return yet
    /// (a fresh upload can take a moment), asked for again next time,
    /// with when they were first reported (Unix seconds).
    #[serde(default)]
    pub pending: Vec<(u64, u64)>,
    pub media: Vec<CachedItem>,
}

/// The fields of a `MediaItem` the CLI uses, minus the download URL,
/// which expires.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CachedItem {
    pub id: u64,
    pub name: Option<String>,
    pub folderid: Option<u64>,
    pub size: Option<u64>,
    pub date: Option<u64>,
    pub creationdate: Option<u64>,
    pub modificationdate: Option<u64>,
}

impl MediaCache {
    pub fn new(account: &str, cursor: u64, media: Vec<CachedItem>) -> Self {
        MediaCache {
            version: VERSION,
            account: account.to_string(),
            cursor,
            listed_at: crate::config::now_unix(),
            pending: Vec::new(),
            media,
        }
    }
}

/// Whether the cache is on (it is unless `O2CLOUD_CACHE` is `0`).
pub fn enabled() -> bool {
    std::env::var("O2CLOUD_CACHE").map_or(true, |v| v != "0")
}

/// `<cache dir>/o2cli/media-cache.json`
fn path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("o2cli").join("media-cache.json"))
}

/// The cache of `account`, unless there is none, it is unreadable, from
/// another version or account, or older than `MAX_AGE_SECS`.
pub fn load(account: &str) -> Option<MediaCache> {
    let json = std::fs::read(path()?).ok()?;
    let cache: MediaCache = serde_json::from_slice(&json).ok()?;
    let fresh = crate::config::now_unix().saturating_sub(cache.listed_at) < MAX_AGE_SECS;
    (cache.version == VERSION && cache.account == account && fresh).then_some(cache)
}

/// Note files just uploaded to `account`: listings can take a minute to
/// include them, so the next update of the cache asks for them by ID
/// (see `pending`), and `pending_count` says when they're all in.
/// Nothing is noted without a cache; the next full listing will do.
pub fn note_uploaded(account: &str, ids: &[u64]) {
    if !enabled() || account.is_empty() || ids.is_empty() {
        return;
    }
    let Some(mut cache) = load(account) else {
        return;
    };
    let mut known: std::collections::HashSet<u64> = cache
        .media
        .iter()
        .map(|m| m.id)
        .chain(cache.pending.iter().map(|(id, _)| *id))
        .collect();
    let now = crate::config::now_unix();
    let before = cache.pending.len();
    for &id in ids {
        if known.insert(id) {
            cache.pending.push((id, now));
        }
    }
    if cache.pending.len() > before {
        save(&cache);
    }
}

/// How many files the cache of `account` is still waiting for the server
/// to return (0 without a cache).
pub fn pending_count(account: &str) -> usize {
    if !enabled() || account.is_empty() {
        return 0;
    }
    load(account).map_or(0, |c| c.pending.len())
}

/// Save the cache. It is only a speed-up, so failing to save it isn't an
/// error. It is written to a temporary file first, so a reader never sees
/// half of it.
pub fn save(cache: &MediaCache) {
    let Some(path) = path() else { return };
    let Some(dir) = path.parent() else { return };
    let Ok(json) = serde_json::to_vec(cache) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!("media-cache.{}.tmp", std::process::id()));
    if crate::config::write_private(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&tmp);
}

/// Delete the cache, if there is one.
pub fn clear() -> std::io::Result<()> {
    match path() {
        Some(path) if path.exists() => std::fs::remove_file(path),
        _ => Ok(()),
    }
}
