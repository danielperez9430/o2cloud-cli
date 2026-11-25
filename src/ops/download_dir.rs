//! `download-dir`: a cloud folder, or the whole account, to a local
//! folder: the reverse of `upload-dir`.
//!
//! The folder is recreated by name inside the output directory (the
//! account's root goes straight into it), with every subfolder, empty
//! ones included. Files uploaded in parts (`name.001`, `name.002`, …)
//! are joined again. Each file is written to `<name>.o2part` and renamed
//! once complete, with its original modification date, so a cut download
//! never looks finished: run the same command again with
//! `--skip-existing` to resume it (from where it stopped, with a Range
//! request). Nothing local is ever overwritten.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{file_parts, print_json, resolve_target, Tree};
use crate::api::{MediaItem, O2Client};
use crate::config::AuthConfig;
use crate::error::O2Error;
use crate::util::format_size;

/// How many files' download URLs are asked for at once, just before
/// downloading them (the server answers 1,000 in under a second).
const URL_BATCH: usize = 200;

pub struct DownloadDirOptions {
    /// Files downloaded at the same time.
    pub jobs: usize,
    /// Skip a file that is already there with the same size, and resume a
    /// cut download.
    pub skip_existing: bool,
    /// Only files with these extensions (see `normalize_extensions`).
    pub extensions: Vec<String>,
    /// Only say what would be downloaded.
    pub dry_run: bool,
}

/// One local file to write: from one cloud file, or from the parts of a
/// file uploaded in parts.
struct Job<'a> {
    pieces: Vec<&'a MediaItem>,
    /// The cloud path, for messages.
    cloud: String,
    /// The local path, relative to the output directory.
    local: PathBuf,
    size: Option<u64>,
    /// Modification date to give the file (Unix ms).
    mtime: Option<u64>,
    /// Whether the local name differs from the cloud one.
    renamed: bool,
}

/// What became of one job.
enum Outcome {
    Downloaded(u64),
    Skipped,
    Failed(O2Error),
}

/// Download the cloud folder `target` (an ID or a path; `/` is the whole
/// account) into `output`.
pub async fn download_dir(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    output: Option<PathBuf>,
    opts: &DownloadDirOptions,
    json: bool,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();
    let folder = resolve_target(Some(target.to_string()), &folders, &folder_name)?;
    let is_root = folders.iter().any(|f| f.id == folder && f.name == "/");
    let out_root = match (output, is_root) {
        (Some(dir), _) => dir,
        (None, false) => PathBuf::from("."),
        (None, true) => {
            return Err(O2Error::Invalid(
                "downloading the whole account needs a folder to put it in: add -o <dir>".into(),
            ))
        }
    };
    let media = client.get_all_media(auth).await?;
    let tree = Tree::new(&folders, &folder_name, &media);
    let top = (!is_root).then(|| folder_name.get(&folder).cloned().unwrap_or_default());
    let (dirs, jobs) = plan(
        &tree,
        &media,
        folder,
        top.as_deref(),
        &opts.extensions,
        cfg!(windows),
    );

    let total: u64 = jobs.iter().filter_map(|j| j.size).sum();
    eprintln!(
        "→ {} {} ({}) — {} files, {} folders, into {}",
        if opts.dry_run {
            "Would download"
        } else {
            "Downloading"
        },
        tree.path(folder),
        format_size(total),
        jobs.len(),
        dirs.len(),
        out_root.display()
    );
    if opts.dry_run {
        if json {
            let entries: Vec<_> = jobs.iter().map(|j| job_json(j, &out_root)).collect();
            print_json(&serde_json::Value::Array(entries));
        }
        return Ok(());
    }

    // The output directory itself too: the root's files go straight into
    // it, and with --ext or no subfolders nothing else would create it.
    for dir in std::iter::once(&PathBuf::new()).chain(&dirs) {
        let path = out_root.join(dir);
        std::fs::create_dir_all(&path).map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot create {}: {}", path.display(), e),
            ))
        })?;
    }

    let stop = AtomicBool::new(false);
    let count = jobs.len();
    // What's already there is settled first, so a rerun asks the server
    // for nothing but what's missing.
    let mut outcomes: Vec<Option<Outcome>> = jobs
        .iter()
        .map(|job| already_there(job, &out_root, opts))
        .collect();
    let pending: Vec<(usize, &Job)> = jobs
        .iter()
        .enumerate()
        .filter(|(i, _)| outcomes[*i].is_none())
        .collect();
    for batch in pending.chunks(URL_BATCH) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let urls = batch_urls(client, auth, batch.iter().map(|(_, job)| *job)).await;
        let urls = match urls {
            Ok(urls) => urls,
            Err(O2Error::SessionExpired) => {
                stop.store(true, Ordering::Relaxed);
                break;
            }
            // Each file then asks for its own URL.
            Err(_) => HashMap::new(),
        };
        use futures_util::StreamExt;
        let (urls, stop, out_root) = (&urls, &stop, &out_root);
        let results: Vec<(usize, Outcome)> = futures_util::stream::iter(batch.iter().copied())
            .map(|(i, job)| {
                async move {
                    if stop.load(Ordering::Relaxed) {
                        return (i, Outcome::Failed(O2Error::SessionExpired));
                    }
                    let outcome = download_job(client, auth, job, out_root, urls, i, count).await;
                    if let Outcome::Failed(e) = &outcome {
                        let fatal = matches!(e, O2Error::SessionExpired)
                            || matches!(e, O2Error::Io(io) if io.kind() == std::io::ErrorKind::StorageFull);
                        if fatal && !stop.swap(true, Ordering::Relaxed) {
                            eprintln!("  ✗ Stopping: {}", e);
                        } else if !fatal {
                            eprintln!("  ⚠ {}: {}", job.cloud, e);
                        }
                    }
                    (i, outcome)
                }
            })
            .buffer_unordered(opts.jobs.max(1))
            .collect()
            .await;
        for (i, outcome) in results {
            outcomes[i] = Some(outcome);
        }
    }

    let (mut done, mut bytes, mut skipped, mut failed) = (0usize, 0u64, 0usize, 0usize);
    let mut entries = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        let mut entry = job_json(job, &out_root);
        match &outcomes[i] {
            Some(Outcome::Downloaded(n)) => {
                done += 1;
                bytes += n;
                entry["action"] = "downloaded".into();
            }
            Some(Outcome::Skipped) => {
                skipped += 1;
                entry["action"] = "skipped".into();
            }
            Some(Outcome::Failed(e)) => {
                failed += 1;
                entry["action"] = "failed".into();
                entry["error"] = e.to_string().into();
            }
            None => {
                failed += 1;
                entry["action"] = "failed".into();
                entry["error"] = "not started: the run stopped".into();
            }
        }
        entries.push(entry);
    }

    if skipped > 0 {
        eprintln!("  ({} file(s) were already there)", skipped);
    }
    if failed > 0 {
        eprintln!("⚠ {} file(s) not downloaded", failed);
    }
    eprintln!(
        "✓ Downloaded {} files ({}) to {}",
        done,
        format_size(bytes),
        out_root.display()
    );
    if json {
        print_json(&serde_json::Value::Array(entries));
    }
    if stop.load(Ordering::Relaxed) {
        return Err(O2Error::Invalid(format!(
            "stopped with {} file(s) left; run the same command with --skip-existing to carry on",
            failed
        )));
    }
    if failed > 0 {
        return Err(O2Error::Api(format!(
            "{} of {} files failed to download",
            failed, count
        )));
    }
    Ok(())
}

/// The folders to create and the files to download, for the cloud folder
/// `folder` (named `top` locally, or the output directory itself for the
/// root): local paths are made safe and unique (see `local_names`).
fn plan<'a>(
    tree: &Tree<'a>,
    media: &'a [MediaItem],
    folder: u64,
    top: Option<&str>,
    extensions: &[String],
    windows: bool,
) -> (Vec<PathBuf>, Vec<Job<'a>>) {
    let mut dirs = Vec::new();
    let mut jobs = Vec::new();
    let base = match top {
        Some(name) => PathBuf::from(local_name(name, folder, true, windows)),
        None => PathBuf::new(),
    };
    if top.is_some() {
        dirs.push(base.clone());
    }
    let mut stack = vec![(folder, base)];
    while let Some((id, local_dir)) = stack.pop() {
        let cloud_dir = tree.path(id);
        let files: Vec<&MediaItem> = tree.files_in(id).collect();
        // Join complete sets of parts, unless a file has the joined name.
        let names: HashSet<&str> = files.iter().filter_map(|m| m.name.as_deref()).collect();
        let mut in_parts: HashSet<u64> = HashSet::new();
        let mut entries: Vec<(u64, String, Entry<'a>)> = Vec::new();
        for m in &files {
            let Some(base) = m.name.as_deref().and_then(|n| n.strip_suffix(".001")) else {
                continue;
            };
            if names.contains(base) {
                continue;
            }
            if let Ok(parts) = file_parts(m, base, media) {
                in_parts.extend(parts.iter().map(|p| p.id));
                entries.push((m.id, base.to_string(), Entry::File(parts)));
            }
        }
        for m in files.into_iter().filter(|m| !in_parts.contains(&m.id)) {
            entries.push((
                m.id,
                m.name.clone().unwrap_or_default(),
                Entry::File(vec![m]),
            ));
        }
        entries.retain(|(_, name, _)| wanted(name, extensions));
        let children: Vec<_> = tree.children(id).collect();
        for child in &children {
            entries.push((child.id, child.name.clone(), Entry::Folder(child.id)));
        }
        let locals = local_names(&entries, windows);
        for ((_, name, entry), local) in entries.into_iter().zip(locals) {
            let path = local_dir.join(&local);
            let cloud = format!("{}/{}", cloud_dir.trim_end_matches('/'), name);
            match entry {
                Entry::Folder(child) => {
                    if extensions.is_empty() {
                        dirs.push(path.clone());
                    }
                    stack.push((child, path));
                }
                Entry::File(pieces) => {
                    let size = pieces.iter().map(|p| p.size).sum::<Option<u64>>();
                    let mtime = pieces[0].modificationdate.or(pieces[0].creationdate);
                    jobs.push(Job {
                        renamed: local != name,
                        pieces,
                        cloud,
                        local: path,
                        size,
                        mtime,
                    });
                }
            }
        }
    }
    // With --ext, only the folders that end up holding a file.
    if !extensions.is_empty() {
        let needed: HashSet<PathBuf> = jobs
            .iter()
            .flat_map(|j| {
                j.local
                    .ancestors()
                    .skip(1)
                    .map(Path::to_path_buf)
                    .collect::<Vec<_>>()
            })
            .collect();
        dirs = needed
            .into_iter()
            .filter(|d| !d.as_os_str().is_empty())
            .collect();
        dirs.sort();
    }
    (dirs, jobs)
}

enum Entry<'a> {
    File(Vec<&'a MediaItem>),
    Folder(u64),
}

/// Whether a file named `name` is kept by `--ext`.
fn wanted(name: &str, extensions: &[String]) -> bool {
    if extensions.is_empty() {
        return true;
    }
    let name = name.to_lowercase();
    extensions.iter().any(|e| {
        name.len() > e.len() + 1
            && name.ends_with(e.as_str())
            && name[..name.len() - e.len()].ends_with('.')
    })
}

/// The local names of the files and folders in one folder (`(id, cloud
/// name, _)`), in the same order: each made safe (see `local_name`), and
/// unique ignoring case, so a download made on one system can be copied
/// to another. The lowest ID keeps a name; the others get ` (<id>)`.
fn local_names(entries: &[(u64, String, Entry<'_>)], windows: bool) -> Vec<String> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&k| entries[k].0);
    let mut used: HashSet<String> = HashSet::new();
    let mut names = vec![String::new(); entries.len()];
    for k in order {
        let (id, name, entry) = &entries[k];
        let is_dir = matches!(entry, Entry::Folder(_));
        let mut local = local_name(name, *id, is_dir, windows);
        if !used.insert(local.to_lowercase()) {
            local = with_id(&local, *id, is_dir);
            used.insert(local.to_lowercase());
        }
        names[k] = local;
    }
    names
}

/// `name` with ` (<id>)` before its extension (`a.jpg` → `a (12).jpg`).
fn with_id(name: &str, id: u64, is_dir: bool) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !is_dir && !stem.is_empty() => format!("{} ({}).{}", stem, id, ext),
        _ => format!("{} ({})", name, id),
    }
}

/// A name from the server made safe to create locally: no directory
/// parts, `.` or `..`, or control characters, and on Windows none of
/// `<>:"|?*`, no trailing dots or spaces, and no reserved device names
/// (`CON`, `NUL`, `COM1`…). Falls back to the ID if nothing is left.
fn local_name(name: &str, id: u64, is_dir: bool, windows: bool) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = last
        .chars()
        .map(|c| {
            if c.is_control() || (windows && "<>:\"|?*".contains(c)) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if windows {
        out = out.trim_end_matches(['.', ' ']).to_string();
        let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if reserved {
            out.insert(0, '_');
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        out = if is_dir {
            format!("folder-{}", id)
        } else {
            format!("{}.bin", id)
        };
    }
    out
}

/// The download URLs of the files in `batch`, in one request.
async fn batch_urls<'a, 'b: 'a>(
    client: &O2Client,
    auth: &AuthConfig,
    batch: impl Iterator<Item = &'a Job<'b>>,
) -> Result<HashMap<u64, String>, O2Error> {
    let ids: Vec<u64> = batch.flat_map(|j| j.pieces.iter().map(|p| p.id)).collect();
    let mut urls = HashMap::new();
    for chunk in ids.chunks(1000) {
        for m in client.get_media_by_ids(chunk, auth).await? {
            if let Some(url) = m.url {
                urls.insert(m.id, url);
            }
        }
    }
    Ok(urls)
}

/// `Some` if the local file is already there: skipped if it has the
/// same size and `--skip-existing` was given, otherwise refused (nothing
/// is overwritten).
fn already_there(job: &Job<'_>, out_root: &Path, opts: &DownloadDirOptions) -> Option<Outcome> {
    let path = out_root.join(&job.local);
    let meta = std::fs::metadata(&path).ok()?;
    if opts.skip_existing && Some(meta.len()) == job.size {
        return Some(Outcome::Skipped);
    }
    let why = if opts.skip_existing {
        " with another size"
    } else {
        " (add --skip-existing to skip it)"
    };
    eprintln!(
        "  ⚠ {}: already exists locally{}; not overwriting it",
        job.cloud, why
    );
    Some(Outcome::Failed(O2Error::Invalid(format!(
        "{} already exists{}; not overwriting it",
        path.display(),
        why
    ))))
}

async fn download_job(
    client: &O2Client,
    auth: &AuthConfig,
    job: &Job<'_>,
    out_root: &Path,
    urls: &HashMap<u64, String>,
    i: usize,
    count: usize,
) -> Outcome {
    let path = out_root.join(&job.local);
    eprintln!(
        "  [{}/{}] 📄 {} ({})",
        i + 1,
        count,
        job.cloud,
        job.size.map(format_size).unwrap_or_else(|| "?".into())
    );
    match fetch(client, auth, job, &path, urls).await {
        Ok(n) => Outcome::Downloaded(n),
        Err(e) => Outcome::Failed(e),
    }
}

/// Download `job` to `path` by way of `<path>.o2part`.
async fn fetch(
    client: &O2Client,
    auth: &AuthConfig,
    job: &Job<'_>,
    path: &Path,
    urls: &HashMap<u64, String>,
) -> Result<u64, O2Error> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let part = path.with_file_name(format!("{}.o2part", file_name));
    let io = |e: std::io::Error| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {}", part.display(), e),
        ))
    };
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part)
        .await
        .map_err(io)?;

    let total = if let [piece] = job.pieces.as_slice() {
        // One file: carry on from what an earlier run left.
        let mut attempt = 0;
        loop {
            let offset = file.metadata().await.map_err(io)?.len();
            let url = match (attempt, urls.get(&piece.id)) {
                (0, Some(url)) => url.clone(),
                _ => client.media_download_url(piece.id, auth).await?,
            };
            match client
                .download_resuming(&url, &mut file, &part, offset, piece.size)
                .await
            {
                Ok(n) => break n,
                // Once more with a fresh URL, from where it got to.
                Err(e)
                    if attempt == 0 && !matches!(e, O2Error::SessionExpired | O2Error::Io(_)) =>
                {
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    } else {
        // Parts: joined from the start each time.
        file.set_len(0).await.map_err(io)?;
        let mut total = 0;
        for piece in &job.pieces {
            let url = match urls.get(&piece.id) {
                Some(url) => url.clone(),
                None => client.media_download_url(piece.id, auth).await?,
            };
            total += client
                .download_into(&url, &mut file, &part, piece.size)
                .await?;
        }
        total
    };
    if let Some(size) = job.size {
        if size != total {
            return Err(O2Error::Api(format!("got {} of {} bytes", total, size)));
        }
    }
    file.sync_all().await.map_err(io)?;
    let file = file.into_std().await;
    if let Some(ms) = job.mtime {
        let _ = file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms));
    }
    drop(file);
    if path.exists() {
        return Err(O2Error::Invalid(format!(
            "{} appeared meanwhile; not overwriting it",
            path.display()
        )));
    }
    std::fs::rename(&part, path).map_err(io)?;
    Ok(total)
}

fn job_json(job: &Job<'_>, out_root: &Path) -> serde_json::Value {
    let local: Vec<String> = job
        .local
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let mut entry = serde_json::json!({
        "name": job.cloud.rsplit('/').next(),
        "cloud": job.cloud,
        "local": local.join("/"),
        "path": out_root.join(&job.local).display().to_string(),
        "folderid": job.pieces[0].folderid,
        "size": job.size,
    });
    if job.pieces.len() > 1 {
        entry["parts"] = job.pieces.iter().map(|p| p.id).collect::<Vec<_>>().into();
    } else {
        entry["id"] = job.pieces[0].id.into();
    }
    if job.renamed {
        entry["renamed"] = true.into();
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_name() {
        assert_eq!(local_name("a.jpg", 1, false, false), "a.jpg");
        assert_eq!(local_name("../../etc/passwd", 1, false, false), "passwd");
        assert_eq!(local_name("..", 7, false, false), "7.bin");
        assert_eq!(local_name("", 7, true, false), "folder-7");
        assert_eq!(local_name("a:b?.txt", 1, false, false), "a:b?.txt");
        assert_eq!(local_name("a:b?.txt", 1, false, true), "a_b_.txt");
        assert_eq!(local_name("notes. ", 1, false, true), "notes");
        assert_eq!(local_name("CON.txt", 1, false, true), "_CON.txt");
        assert_eq!(local_name("com3", 1, true, true), "_com3");
        assert_eq!(local_name("COMMENTS.txt", 1, false, true), "COMMENTS.txt");
        assert_eq!(local_name("tab\there", 1, false, false), "tab_here");
    }

    #[test]
    fn test_local_names_unique_ignoring_case() {
        let e = |id, name: &str, dir: bool| {
            (
                id,
                name.to_string(),
                if dir {
                    Entry::Folder(id)
                } else {
                    Entry::File(Vec::new())
                },
            )
        };
        let entries = vec![
            e(30, "Photo.JPG", false),
            e(10, "photo.jpg", false),
            e(20, "photo.jpg", true),
            e(40, "x", false),
        ];
        assert_eq!(
            local_names(&entries, false),
            ["Photo (30).JPG", "photo.jpg", "photo.jpg (20)", "x"]
        );
    }

    #[test]
    fn test_wanted() {
        let exts = vec!["jpg".to_string(), "tar.gz".to_string()];
        assert!(wanted("A.JPG", &exts));
        assert!(wanted("b.tar.gz", &exts));
        assert!(!wanted("jpg", &exts));
        assert!(!wanted("c.png", &exts));
        assert!(wanted("anything", &[]));
    }

    fn item(id: u64, name: &str, folder: u64, size: u64) -> MediaItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "folder": folder, "size": size, "modificationdate": 1_600_000_000_000u64
        }))
        .unwrap()
    }

    /// A path with `/` between its parts on every system.
    fn slashed(p: &Path) -> String {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    #[test]
    fn test_plan() {
        let f = |id, name: &str, parentid| crate::api::FolderEntry {
            id,
            name: name.into(),
            parentid,
        };
        let folders = vec![
            f(1, "/", 0),
            f(2, "Fotos", 1),
            f(3, "Vacío", 2),
            f(4, "2024", 2),
        ];
        let folder_name: HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();
        let media = vec![
            item(100, "a.jpg", 2, 10),
            item(101, "big.iso.001", 2, 100),
            item(102, "big.iso.002", 2, 30),
            item(103, "b.png", 4, 5),
            item(104, "root.txt", 1, 1),
            serde_json::from_value(serde_json::json!({"id": 105, "name": "phone.jpg", "size": 7}))
                .unwrap(),
        ];
        let tree = Tree::new(&folders, &folder_name, &media);

        let (dirs, jobs) = plan(&tree, &media, 2, Some("Fotos"), &[], false);
        let mut dirs: Vec<String> = dirs.iter().map(|d| slashed(d)).collect();
        dirs.sort();
        assert_eq!(dirs, ["Fotos", "Fotos/2024", "Fotos/Vacío"]);
        let mut got: Vec<(String, Option<u64>, usize)> = jobs
            .iter()
            .map(|j| (slashed(&j.local), j.size, j.pieces.len()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("Fotos/2024/b.png".to_string(), Some(5), 1),
                ("Fotos/a.jpg".to_string(), Some(10), 1),
                ("Fotos/big.iso".to_string(), Some(130), 2),
            ]
        );
        assert!(jobs.iter().all(|j| j.mtime == Some(1_600_000_000_000)));

        // The root: straight into the output, phone-app files included.
        let (_, jobs) = plan(&tree, &media, 1, None, &[], false);
        let locals: HashSet<String> = jobs.iter().map(|j| slashed(&j.local)).collect();
        assert!(
            locals.contains("root.txt")
                && locals.contains("phone.jpg")
                && locals.contains("Fotos/a.jpg")
        );

        // --ext: only matching files, and only the folders that hold them.
        let (dirs, jobs) = plan(&tree, &media, 2, Some("Fotos"), &["png".to_string()], false);
        let dirs: Vec<String> = dirs.iter().map(|d| slashed(d)).collect();
        assert_eq!(dirs, ["Fotos", "Fotos/2024"]);
        assert_eq!(jobs.len(), 1);
    }
}
