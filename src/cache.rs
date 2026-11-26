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
    /// What this CLI knows of files it uploaded that are still pending
    /// (name, folder, size, dates: what it sent). Listings show them
    /// until the server returns them, so a file shows up as soon as it's
    /// uploaded rather than 10–60 s later.
    #[serde(default)]
    pub provisional: Vec<CachedItem>,
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
            provisional: Vec::new(),
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

/// Note files just uploaded to `account` (as sent): listings can take a
/// minute to include them, so the next update of the cache asks for them
/// by ID (see `pending`) and lists them meanwhile as uploaded (see
/// `provisional`). Nothing is noted without a cache; the next full
/// listing will do.
pub fn note_uploaded(account: &str, items: &[CachedItem]) {
    if !enabled() || account.is_empty() || items.is_empty() {
        return;
    }
    let Some(mut cache) = load(account) else {
        return;
    };
    if note(&mut cache, items, crate::config::now_unix()) {
        save(&cache);
    }
}

/// `note_uploaded` on a loaded cache; whether anything was added.
fn note(cache: &mut MediaCache, items: &[CachedItem], now: u64) -> bool {
    let mut known: std::collections::HashSet<u64> = cache
        .media
        .iter()
        .map(|m| m.id)
        .chain(cache.pending.iter().map(|(id, _)| *id))
        .collect();
    let mut added = false;
    for item in items {
        if known.insert(item.id) {
            cache.pending.push((item.id, now));
            cache.provisional.push(item.clone());
            added = true;
        }
    }
    added
}

/// How many files the cache of `account` is still waiting for the server
/// to return without knowing what they are (0 without a cache). Files
/// this CLI uploaded aren't counted: it lists them as uploaded.
pub fn pending_count(account: &str) -> usize {
    if !enabled() || account.is_empty() {
        return 0;
    }
    load(account).map_or(0, |c| unknown_pending(&c))
}

fn unknown_pending(cache: &MediaCache) -> usize {
    cache
        .pending
        .iter()
        .filter(|(id, _)| !cache.provisional.iter().any(|p| p.id == *id))
        .count()
}

/// The IDs of files uploaded by this CLI that the server doesn't list yet.
pub fn provisional_ids(account: &str) -> Vec<u64> {
    if !enabled() || account.is_empty() {
        return Vec::new();
    }
    load(account).map_or_else(Vec::new, |c| {
        c.provisional
            .iter()
            .filter(|p| c.pending.iter().any(|(id, _)| *id == p.id))
            .map(|p| p.id)
            .collect()
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64) -> CachedItem {
        CachedItem {
            id,
            name: Some(format!("f{id}.txt")),
            folderid: Some(9),
            size: Some(3),
            date: Some(1),
            creationdate: Some(1),
            modificationdate: Some(1),
        }
    }

    #[test]
    fn test_note_uploaded_is_provisional() {
        let mut cache = MediaCache::new("600", 0, vec![item(1)]);
        // A file already listed isn't noted again; a new one is, both as
        // pending and as what was sent.
        assert!(note(&mut cache, &[item(1), item(2)], 50));
        assert_eq!(cache.pending, vec![(2, 50)]);
        assert_eq!(cache.provisional, vec![item(2)]);
        assert!(!note(&mut cache, &[item(2)], 60));
        // Uploaded here: not waited for. Reported by the change log only:
        // waited for.
        assert_eq!(unknown_pending(&cache), 0);
        cache.pending.push((3, 70));
        assert_eq!(unknown_pending(&cache), 1);
    }
}
