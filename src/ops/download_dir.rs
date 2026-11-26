//! `download-dir`: a cloud folder, or the whole account, to a local
//! folder: the reverse of `upload-dir`.
//!
//! The folder is recreated by name inside the output directory (the
//! account's root goes straight into it), with every subfolder, empty
//! ones included. Files uploaded in parts (`name.001`, `name.002`, …)
//! are joined again. Each file is written to `.<id>.o2part` and renamed
//! once complete, with its original modification date, so a cut download
//! never looks finished: run the same command again with
//! (it's the default since 1.0) to resume it (from where it stopped, with a Range
//! request). Nothing local is ever overwritten.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{file_parts, made_by_cli, print_json, resolve_target, Tree};
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
    /// Not started: the run stopped first.
    NotStarted,
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
        folder,
        top.as_deref(),
        &opts.extensions,
        cfg!(windows),
    );
    run_jobs(
        client,
        auth,
        &tree.path(folder),
        dirs,
        jobs,
        out_root,
        opts,
        json,
    )
    .await
}

/// Download the photos and videos of a label (`media`, named `name`: a
/// moment, a place, a person, an album) into a folder of that name inside
/// `output`.
pub async fn download_label(
    client: &O2Client,
    auth: &AuthConfig,
    name: &str,
    media: &[MediaItem],
    output: Option<PathBuf>,
    opts: &DownloadDirOptions,
    json: bool,
) -> Result<(), O2Error> {
    let out_root = output.unwrap_or_else(|| PathBuf::from("."));
    let (dirs, jobs) = plan_flat(media, name, &opts.extensions, cfg!(windows));
    run_jobs(client, auth, name, dirs, jobs, out_root, opts, json).await
}

/// The folder (`top`) and the files to download for a flat list of media
/// (a label's), with local names made safe and unique as `plan` does.
fn plan_flat<'a>(
    media: &'a [MediaItem],
    top: &str,
    extensions: &[String],
    windows: bool,
) -> (Vec<PathBuf>, Vec<Job<'a>>) {
    let base = PathBuf::from(local_name(top, 0, true, windows));
    let mut entries: Vec<(u64, String, Entry<'a>)> = media
        .iter()
        .map(|m| {
            (
                m.id,
                m.name.clone().unwrap_or_default(),
                Entry::File(vec![m]),
            )
        })
        .collect();
    entries.retain(|(_, name, _)| wanted(name, extensions));
    let locals = local_names(&entries, windows);
    let mut jobs = Vec::new();
    for ((_, name, entry), local) in entries.into_iter().zip(locals) {
        if let Entry::File(pieces) = entry {
            let size = pieces.iter().map(|p| p.size).sum::<Option<u64>>();
            let mtime = pieces[0].modificationdate.or(pieces[0].creationdate);
            jobs.push(Job {
                renamed: local != name,
                pieces,
                cloud: format!("{}/{}", top, name),
                local: base.join(&local),
                size,
                mtime,
            });
        }
    }
    (vec![base], jobs)
}

/// Download `jobs` (from `source`, for messages) into `out_root`, creating
/// `dirs` first.
#[allow(clippy::too_many_arguments)]
async fn run_jobs(
    client: &O2Client,
    auth: &AuthConfig,
    source: &str,
    dirs: Vec<PathBuf>,
    jobs: Vec<Job<'_>>,
    out_root: PathBuf,
    opts: &DownloadDirOptions,
    json: bool,
) -> Result<(), O2Error> {
    let total: u64 = jobs.iter().filter_map(|j| j.size).sum();
    crate::events::emit(
        "start",
        serde_json::json!({
            "command": "download-dir", "files": jobs.len(), "bytes": total,
            "dry_run": opts.dry_run, "schema": crate::events::SCHEMA, "run": crate::log::run_id(),
        }),
    );
    crate::info!(
        "→ {} {} ({}) — {} files, {} folders, into {}",
        if opts.dry_run {
            "Would download"
        } else {
            "Downloading"
        },
        source,
        format_size(total),
        jobs.len(),
        dirs.len(),
        out_root.display()
    );
    if opts.dry_run {
        // What the real run would do with what's on disk now.
        let (mut new, mut resume, mut there, mut in_the_way) = (0usize, 0usize, 0usize, 0usize);
        let (mut to_get, mut entries) = (0u64, Vec::new());
        for job in &jobs {
            let mut entry = job_json(job, &out_root);
            entry["dry_run"] = true.into();
            let size = job.size.unwrap_or(0);
            let action = match local_state(job, &out_root, opts) {
                Local::Missing => {
                    new += 1;
                    to_get += size;
                    "download"
                }
                Local::Partial(have) => {
                    resume += 1;
                    to_get += size.saturating_sub(have);
                    "resume"
                }
                Local::Same => {
                    there += 1;
                    "skipped"
                }
                Local::InTheWay => {
                    in_the_way += 1;
                    crate::warn!(
                        "  ⚠ {}: already exists locally{}; it would be left alone",
                        job.cloud,
                        if opts.skip_existing {
                            " with another size"
                        } else {
                            " (add --skip-existing to skip it)"
                        }
                    );
                    "failed"
                }
            };
            entry["action"] = action.into();
            crate::events::emit("file", entry.clone());
            if json {
                entries.push(entry);
            }
        }
        crate::info!(
            "Dry run: {} to download, {} to resume, {} already there, {} in the way \
             ({} to transfer); nothing was changed.",
            new,
            resume,
            there,
            in_the_way,
            format_size(to_get)
        );
        crate::events::emit(
            "summary",
            serde_json::json!({
                "dry_run": true, "download": new, "resume": resume, "skipped": there,
                "in_the_way": in_the_way, "bytes": to_get,
            }),
        );
        if json {
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

    let bar = crate::ui::start("Downloading", jobs.len() as u64, total);
    let stop = AtomicBool::new(false);
    // Why it stopped, if it did: the session, or else a full disk.
    let session_over = AtomicBool::new(false);
    let session_over = &session_over;
    let count = jobs.len();
    // What's already there is settled first, so a rerun asks the server
    // for nothing but what's missing.
    let mut outcomes: Vec<Option<Outcome>> = jobs
        .iter()
        .map(|job| already_there(job, &out_root, opts))
        .collect();
    for (job, outcome) in jobs.iter().zip(&outcomes) {
        if let Some(outcome) = outcome {
            crate::ui::file_done(job.size.unwrap_or(0), false);
            crate::events::emit("file", described(job, &out_root, Some(outcome)));
        }
    }
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
                crate::warn!("  ✗ Stopping: {}", O2Error::SessionExpired);
                stop.store(true, Ordering::Relaxed);
                session_over.store(true, Ordering::Relaxed);
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
                        let outcome = Outcome::NotStarted;
                        crate::events::emit("file", described(job, out_root, Some(&outcome)));
                        return (i, outcome);
                    }
                    let key = serde_json::json!({ "local": job_json(job, out_root)["local"] });
                    let outcome = crate::events::track(
                        key,
                        job.size.unwrap_or(0),
                        download_job(client, auth, job, out_root, urls, i, count),
                    )
                    .await;
                    if crate::events::on() {
                        crate::events::emit("file", described(job, out_root, Some(&outcome)));
                    }
                    if let Outcome::Failed(e) = &outcome {
                        let fatal = matches!(e, O2Error::SessionExpired)
                            || matches!(e, O2Error::Io(io) if io.kind() == std::io::ErrorKind::StorageFull);
                        if matches!(e, O2Error::SessionExpired) {
                            session_over.store(true, Ordering::Relaxed);
                        }
                        if fatal && !stop.swap(true, Ordering::Relaxed) {
                            crate::warn!("  ✗ Stopping: {}", e);
                        } else if !fatal {
                            let e = crate::redact::text(&e.to_string());
                            crate::warn!("  ⚠ {}: {}", job.cloud, e);
                            crate::log::warn(
                                "download-dir",
                                "file failed",
                                serde_json::json!({ "cloud": job.cloud, "error": e }),
                            );
                        }
                    }
                    (i, outcome)
                }
            })
            .buffer_unordered(opts.jobs.max(1))
            .collect()
            .await;
        for (i, outcome) in results {
            let moved = matches!(outcome, Outcome::Downloaded(_));
            crate::ui::file_done(jobs[i].size.unwrap_or(0), moved);
            outcomes[i] = Some(outcome);
        }
    }
    drop(bar);

    let (mut done, mut bytes, mut skipped, mut failed) = (0usize, 0u64, 0usize, 0usize);
    let mut entries = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        match &outcomes[i] {
            Some(Outcome::Downloaded(n)) => {
                done += 1;
                bytes += n;
            }
            Some(Outcome::Skipped) => skipped += 1,
            Some(Outcome::Failed(_)) | Some(Outcome::NotStarted) => failed += 1,
            None => {
                failed += 1;
                // Never started: said once here.
                crate::events::emit("file", described(job, &out_root, None));
            }
        }
        if json {
            entries.push(described(job, &out_root, outcomes[i].as_ref()));
        }
    }

    crate::events::emit(
        "summary",
        serde_json::json!({
            "files": count, "downloaded": done, "bytes": bytes, "skipped": skipped,
            "failed": failed,
        }),
    );
    crate::log::info(
        "download-dir",
        "done",
        serde_json::json!({
            "files": count, "downloaded": done, "bytes": bytes, "skipped": skipped,
            "failed": failed,
        }),
    );
    if skipped > 0 {
        crate::info!("  ({} file(s) were already there)", skipped);
    }
    if failed > 0 {
        crate::warn!("⚠ {} file(s) not downloaded", failed);
    }
    crate::info!(
        "✓ Downloaded {} files ({}) to {}",
        done,
        format_size(bytes),
        out_root.display()
    );
    if json {
        print_json(&serde_json::Value::Array(entries));
    }
    if session_over.load(Ordering::Relaxed) {
        return Err(O2Error::SessionExpired);
    }
    if stop.load(Ordering::Relaxed) {
        return Err(O2Error::Io(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            format!(
                "stopped with {} file(s) left: the disk is full. Make room and run the \
                 same command again to carry on",
                failed
            ),
        )));
    }
    if failed > 0 {
        return Err(O2Error::Partial(format!(
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
            // Among this folder's files only: the account can be large.
            // Only the CLI's parts: someone's own `x.001`, `x.002`…
            // volumes come down as they are, as they were uploaded.
            if let Ok(parts) = file_parts(m, base, files.iter().copied())
                .map_err(|_| ())
                .and_then(|p| if made_by_cli(&p) { Ok(p) } else { Err(()) })
            {
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

/// What is on disk for a job.
#[derive(Debug, PartialEq)]
enum Local {
    /// Nothing: downloaded from the start.
    Missing,
    /// A cut download (`.<id>.o2part`) with these bytes: resumed.
    Partial(u64),
    /// The file, with the same size, and `--skip-existing`: skipped.
    Same,
    /// The file, of another size or without `--skip-existing`: left alone.
    InTheWay,
}

fn local_state(job: &Job<'_>, out_root: &Path, opts: &DownloadDirOptions) -> Local {
    let path = out_root.join(&job.local);
    if let Ok(meta) = std::fs::metadata(&path) {
        return if opts.skip_existing && Some(meta.len()) == job.size {
            Local::Same
        } else {
            Local::InTheWay
        };
    }
    let part = path.with_file_name(format!(".{}.o2part", job.pieces[0].id));
    match std::fs::metadata(&part) {
        Ok(meta) if meta.len() > 0 && job.size.is_none_or(|size| meta.len() <= size) => {
            Local::Partial(meta.len())
        }
        _ => Local::Missing,
    }
}

/// `Some` if the local file is already there: skipped if it has the
/// same size and `--skip-existing` was given, otherwise refused (nothing
/// is overwritten).
fn already_there(job: &Job<'_>, out_root: &Path, opts: &DownloadDirOptions) -> Option<Outcome> {
    match local_state(job, out_root, opts) {
        Local::Same => return Some(Outcome::Skipped),
        Local::InTheWay => {}
        Local::Missing | Local::Partial(_) => return None,
    }
    let path = out_root.join(&job.local);
    let why = if opts.skip_existing {
        " with another size"
    } else {
        " (add --skip-existing to skip it)"
    };
    crate::warn!(
        "  ⚠ {}: already exists locally{}; not overwriting it",
        job.cloud,
        why
    );
    Some(Outcome::Failed(O2Error::Conflict(format!(
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
    crate::detail!(
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

/// Open (or create) a partial download for writing, keeping what it has.
/// Not in append mode: on Windows a file opened so can't be truncated,
/// which a download that starts over needs.
pub(super) fn open_part(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
}

/// Download `job` to `path` by way of `.<id>.o2part` next to it.
async fn fetch(
    client: &O2Client,
    auth: &AuthConfig,
    job: &Job<'_>,
    path: &Path,
    urls: &HashMap<u64, String>,
) -> Result<u64, O2Error> {
    // Named after the cloud file's ID, so a leftover from another version
    // of the file is never resumed into this one (and short, so a long
    // name still fits).
    let part = path.with_file_name(format!(".{}.o2part", job.pieces[0].id));
    let io = |e: std::io::Error| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {}", part.display(), e),
        ))
    };
    let mut file = tokio::fs::File::from_std(open_part(&part).map_err(io)?);
    let total = fetch_pieces(client, auth, &job.pieces, job.size, &mut file, &part, urls).await?;
    finish(file, job.mtime, &part).await?;
    if path.exists() {
        return Err(O2Error::Conflict(format!(
            "{} appeared meanwhile; not overwriting it",
            path.display()
        )));
    }
    std::fs::rename(&part, path).map_err(io)?;
    Ok(total)
}

/// Write `pieces` (one file, or the parts of a file uploaded in parts;
/// `size` bytes in all, if known) into `file` (at `path`, for messages),
/// carrying on from what is already in it: whole pieces already there
/// are skipped and the one it stopped in is resumed with Range. Each is
/// retried once, with a fresh URL (they expire), from where it got to.
/// `urls` are download URLs already at hand. Returns the file's size.
pub(super) async fn fetch_pieces(
    client: &O2Client,
    auth: &AuthConfig,
    pieces: &[&MediaItem],
    size: Option<u64>,
    file: &mut tokio::fs::File,
    path: &Path,
    urls: &HashMap<u64, String>,
) -> Result<u64, O2Error> {
    let io = |e: std::io::Error| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {}", path.display(), e),
        ))
    };
    let mut have = file.metadata().await.map_err(io)?.len();
    if size.is_some_and(|size| have > size) {
        file.set_len(0).await.map_err(io)?;
        have = 0;
    }
    let mut start = 0u64;
    for piece in pieces {
        let len = piece.size;
        if let Some(len) = len {
            if have >= start + len {
                start += len;
                continue;
            }
        }
        let mut attempt = 0;
        loop {
            have = file.metadata().await.map_err(io)?.len();
            let offset = have.saturating_sub(start);
            let url = match (attempt, urls.get(&piece.id)) {
                (0, Some(url)) => url.clone(),
                _ => client.media_download_url(piece.id, auth).await?,
            };
            match client
                .download_resuming(&url, file, path, start, offset, len)
                .await
            {
                Ok(n) => {
                    start += n;
                    break;
                }
                Err(e)
                    if attempt == 0 && !matches!(e, O2Error::SessionExpired | O2Error::Io(_)) =>
                {
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
    if let Some(size) = size {
        if size != start {
            return Err(O2Error::Api(format!("got {} of {} bytes", start, size)));
        }
    }
    Ok(start)
}

/// Flush a finished download to disk and give it its date (`mtime`,
/// Unix ms).
pub(super) async fn finish(
    file: tokio::fs::File,
    mtime: Option<u64>,
    path: &Path,
) -> Result<(), O2Error> {
    file.sync_all().await.map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {}", path.display(), e),
        ))
    })?;
    let file = file.into_std().await;
    if let Some(ms) = mtime {
        let _ = file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms));
    }
    Ok(())
}

/// A job's `--json` entry with what became of it (`None`: it never
/// started, because the run stopped).
fn described(job: &Job<'_>, out_root: &Path, outcome: Option<&Outcome>) -> serde_json::Value {
    let mut entry = job_json(job, out_root);
    match outcome {
        Some(Outcome::Downloaded(_)) => entry["action"] = "downloaded".into(),
        Some(Outcome::Skipped) => entry["action"] = "skipped".into(),
        Some(Outcome::Failed(e)) => {
            entry["action"] = "failed".into();
            entry["error"] = crate::redact::text(&e.to_string()).into();
        }
        Some(Outcome::NotStarted) | None => {
            entry["action"] = "failed".into();
            entry["error"] = "not started: the run stopped".into();
        }
    }
    entry
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
    let ids: Vec<u64> = job.pieces.iter().map(|p| p.id).collect();
    if ids.len() > 1 {
        entry["parts"] = ids.clone().into();
    } else {
        entry["id"] = ids[0].into();
    }
    entry["ids"] = ids.into();
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
    fn test_open_part_keeps_and_truncates() {
        use std::io::{Read, Seek, Write};
        let path = std::env::temp_dir().join(format!("o2cloud-test-{}.o2part", std::process::id()));
        let _ = std::fs::remove_file(&path);
        open_part(&path).unwrap().write_all(b"hello").unwrap();
        // Reopened: kept, and writable at the end.
        let mut f = open_part(&path).unwrap();
        f.seek(std::io::SeekFrom::End(0)).unwrap();
        f.write_all(b" world").unwrap();
        // And it can be cut back, as a restart does.
        f.set_len(5).unwrap();
        drop(f);
        let mut got = String::new();
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(got, "hello");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_local_state() {
        let dir = std::env::temp_dir().join(format!("o2cli_local_state_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let piece = item(7, "f.bin", 2, 10);
        let job = Job {
            pieces: vec![&piece],
            cloud: "/f.bin".into(),
            local: PathBuf::from("f.bin"),
            size: Some(10),
            mtime: None,
            renamed: false,
        };
        let opts = |skip_existing| DownloadDirOptions {
            jobs: 1,
            skip_existing,
            extensions: Vec::new(),
            dry_run: true,
        };
        assert_eq!(local_state(&job, &dir, &opts(true)), Local::Missing);
        std::fs::write(dir.join(".7.o2part"), b"1234").unwrap();
        assert_eq!(local_state(&job, &dir, &opts(true)), Local::Partial(4));
        // Bigger than the file: started over, as the real run does.
        std::fs::write(dir.join(".7.o2part"), b"12345678901").unwrap();
        assert_eq!(local_state(&job, &dir, &opts(true)), Local::Missing);
        std::fs::write(dir.join("f.bin"), b"0123456789").unwrap();
        assert_eq!(local_state(&job, &dir, &opts(true)), Local::Same);
        assert_eq!(local_state(&job, &dir, &opts(false)), Local::InTheWay);
        std::fs::write(dir.join("f.bin"), b"012").unwrap();
        assert_eq!(local_state(&job, &dir, &opts(true)), Local::InTheWay);
        std::fs::remove_dir_all(&dir).unwrap();
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
    fn test_plan_flat() {
        let media = vec![
            item(1, "IMG_1.JPG", 2, 10),
            item(2, "img_1.jpg", 3, 20),
            item(3, "b.png", 4, 5),
        ];
        let (dirs, jobs) = plan_flat(&media, "Un día: hace 1 año", &[], true);
        assert_eq!(dirs.len(), 1);
        let locals: Vec<String> = jobs.iter().map(|j| slashed(&j.local)).collect();
        // Into one folder named after the label (made safe for Windows),
        // and two names that differ only in case kept apart.
        assert!(locals.iter().all(|l| l.starts_with(&slashed(&dirs[0]))));
        assert!(!slashed(&dirs[0]).contains(':'), "{:?}", dirs);
        let mut names: Vec<String> = locals.iter().map(|l| l.to_lowercase()).collect();
        names.dedup();
        assert_eq!(names.len(), 3, "{locals:?}");
        let (_, jobs) = plan_flat(&media, "x", &["png".to_string()], false);
        assert_eq!(jobs.len(), 1);
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
            // The CLI's parts: joined.
            item(101, "big.iso.001", 2, super::super::DEFAULT_PART_SIZE),
            item(102, "big.iso.002", 2, 30),
            // Someone's own volumes, uploaded as files: left as they are.
            item(106, "backup.7z.001", 2, 100),
            item(107, "backup.7z.002", 2, 40),
            item(103, "b.png", 4, 5),
            item(104, "root.txt", 1, 1),
            serde_json::from_value(serde_json::json!({"id": 105, "name": "phone.jpg", "size": 7}))
                .unwrap(),
        ];
        let tree = Tree::new(&folders, &folder_name, &media);

        let (dirs, jobs) = plan(&tree, 2, Some("Fotos"), &[], false);
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
                ("Fotos/backup.7z.001".to_string(), Some(100), 1),
                ("Fotos/backup.7z.002".to_string(), Some(40), 1),
                (
                    "Fotos/big.iso".to_string(),
                    Some(super::super::DEFAULT_PART_SIZE + 30),
                    2
                ),
            ]
        );
        assert!(jobs.iter().all(|j| j.mtime == Some(1_600_000_000_000)));

        // The root: straight into the output, phone-app files included.
        let (_, jobs) = plan(&tree, 1, None, &[], false);
        let locals: HashSet<String> = jobs.iter().map(|j| slashed(&j.local)).collect();
        assert!(
            locals.contains("root.txt")
                && locals.contains("phone.jpg")
                && locals.contains("Fotos/a.jpg")
        );

        // --ext: only matching files, and only the folders that hold them.
        let (dirs, jobs) = plan(&tree, 2, Some("Fotos"), &["png".to_string()], false);
        let dirs: Vec<String> = dirs.iter().map(|d| slashed(d)).collect();
        assert_eq!(dirs, ["Fotos", "Fotos/2024"]);
        assert_eq!(jobs.len(), 1);
    }
}
