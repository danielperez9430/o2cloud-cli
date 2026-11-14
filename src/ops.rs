use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::api::{O2Client, UploadMeta};
use crate::config::AuthConfig;
use crate::error::O2Error;
use crate::util::{format_size, format_unix_utc, mime_type_from_name, system_time_to_compact_iso};

// ------------------------------------------------------------------
// List
// ------------------------------------------------------------------

/// How `ls` orders files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SortKey {
    Name,
    Size,
    Date,
}

/// Display options for `ls`.
#[derive(Debug, Clone)]
pub struct ListOptions {
    /// Every file under the folder, recursively, as a flat list.
    pub all: bool,
    /// Hierarchical tree view.
    pub tree: bool,
    pub sort: SortKey,
    pub reverse: bool,
    /// Also show each file's date.
    pub long: bool,
    /// Only items whose name matches this `*`/`?` pattern.
    pub pattern: Option<String>,
    /// Show at most this many files.
    pub limit: Option<usize>,
    pub json: bool,
}

/// List files with multiple display modes:
/// - Default: folder contents (direct children), root if no `path`
/// - `path`: folder path (e.g. `/Photos/2024`) or ID (e.g. `12345678`)
/// - `all`: every file under the folder, as a flat list
/// - `tree`: hierarchical tree view
pub async fn list_folder(
    client: &O2Client,
    auth: &AuthConfig,
    path: Option<String>,
    opts: &ListOptions,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = client.get_all_media(auth).await?;

    if media.is_empty() && folders.is_empty() {
        if opts.json {
            println!("[]");
        } else {
            println!("(empty)");
        }
        return Ok(());
    }

    // Build lookup tables
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    // Resolve target folder
    let target = resolve_target(path, &folders, &folder_name)?;

    if opts.tree {
        print_tree(&folders, &media, &folder_name, target, "", true, opts);
    } else if opts.all {
        print_flat_all(&media, &folder_name, &folders, target, opts);
    } else {
        print_folder_contents(&folders, &media, &folder_name, target, opts);
    }

    Ok(())
}

// ------------------------------------------------------------------
// Display helpers
// ------------------------------------------------------------------

/// Whether `item` is listed in folder `folder`. Files without a folder,
/// or whose folder isn't in the folder list (the server can return a
/// fresh upload like that while it's still processing it), are listed in
/// the root, as `find` already shows them — otherwise they'd appear
/// nowhere.
fn shown_in_folder(
    item: &crate::api::MediaItem,
    folder: u64,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
) -> bool {
    match item.folderid {
        Some(fid) if folder_name.contains_key(&fid) => fid == folder,
        _ => is_root_folder(folder, folders),
    }
}

/// Case-insensitive match of `name` against a pattern where `*` matches
/// any run of characters and `?` exactly one.
fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    // Where the last `*` was, and the name position it's matched up to
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            // Let the last `*` swallow one more character
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn name_matches(name: &str, opts: &ListOptions) -> bool {
    opts.pattern.as_deref().is_none_or(|p| glob_match(p, name))
}

/// The date shown and sorted on: when the file was created (for a photo,
/// when it was taken), or else when it was uploaded. Unix milliseconds.
fn file_date(item: &crate::api::MediaItem) -> Option<u64> {
    item.creationdate.or(item.date)
}

fn format_date(ms: Option<u64>) -> String {
    match ms {
        Some(ms) => {
            let s = format_unix_utc(ms / 1000);
            s.trim_end_matches(" UTC").to_string()
        }
        None => "-".repeat(16),
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for JSON output.
pub fn format_date_iso(ms: Option<u64>) -> serde_json::Value {
    match ms {
        Some(ms) => {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
            let c = system_time_to_compact_iso(t); // YYYYMMDDTHHMMSS
            format!(
                "{}-{}-{}T{}:{}:{}Z",
                &c[0..4],
                &c[4..6],
                &c[6..8],
                &c[9..11],
                &c[11..13],
                &c[13..15]
            )
            .into()
        }
        None => serde_json::Value::Null,
    }
}

/// Sort files by `opts.sort`, ties broken by name.
fn sort_files(files: &mut [&crate::api::MediaItem], opts: &ListOptions) {
    let by_name = |a: &&crate::api::MediaItem, b: &&crate::api::MediaItem| {
        let (a, b) = (
            a.name.as_deref().unwrap_or(""),
            b.name.as_deref().unwrap_or(""),
        );
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    };
    files.sort_by(|a, b| {
        let primary = match opts.sort {
            SortKey::Name => std::cmp::Ordering::Equal,
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Date => file_date(a).cmp(&file_date(b)),
        };
        primary.then_with(|| by_name(a, b))
    });
    if opts.reverse {
        files.reverse();
    }
}

fn sort_folders(folders: &mut [&crate::api::FolderEntry]) {
    folders.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Number of files and their total size in `folder_id` and below.
fn folder_totals(
    folder_id: u64,
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
) -> (usize, u64) {
    let ids = subtree_ids(folder_id, folders);
    media
        .iter()
        .filter(|m| in_subtree(m, &ids, folders, folder_name))
        .fold((0, 0), |(n, size), m| (n + 1, size + m.size.unwrap_or(0)))
}

/// `folder_id` and every folder below it.
fn subtree_ids(
    folder_id: u64,
    folders: &[crate::api::FolderEntry],
) -> std::collections::HashSet<u64> {
    let mut ids = std::collections::HashSet::from([folder_id]);
    let mut stack = vec![folder_id];
    while let Some(id) = stack.pop() {
        for child in folders.iter().filter(|f| f.parentid == id) {
            if ids.insert(child.id) {
                stack.push(child.id);
            }
        }
    }
    ids
}

/// Whether `item` is in one of the folders `ids` (see `shown_in_folder`
/// for files without a known folder).
fn in_subtree(
    item: &crate::api::MediaItem,
    ids: &std::collections::HashSet<u64>,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
) -> bool {
    match item.folderid {
        Some(fid) if folder_name.contains_key(&fid) => ids.contains(&fid),
        _ => ids.iter().any(|&id| is_root_folder(id, folders)),
    }
}

/// Apply the name filter, sort, then the limit. Returns the files to
/// show and how many matched in total.
fn select_files<'a>(
    files: impl Iterator<Item = &'a crate::api::MediaItem>,
    opts: &ListOptions,
) -> (Vec<&'a crate::api::MediaItem>, usize, u64) {
    let mut files: Vec<_> = files
        .filter(|m| name_matches(m.name.as_deref().unwrap_or(""), opts))
        .collect();
    let total = files.len();
    let total_size = files.iter().map(|m| m.size.unwrap_or(0)).sum();
    sort_files(&mut files, opts);
    if let Some(limit) = opts.limit {
        files.truncate(limit);
    }
    (files, total, total_size)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

fn file_json(item: &crate::api::MediaItem, path: Option<String>) -> serde_json::Value {
    let mut v = serde_json::json!({
        "id": item.id,
        "name": item.name,
        "size": item.size,
        "date": format_date_iso(file_date(item)),
    });
    if let Some(path) = path {
        v["path"] = path.into();
    }
    v
}

/// Print one JSON value to stdout, for `--json`.
pub fn print_json(v: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
    );
}

/// Footer for the table views: what matched, its size, and how many
/// files weren't shown.
fn print_summary(folders: usize, shown: usize, total: usize, total_size: u64) {
    let mut parts = Vec::new();
    if folders > 0 {
        parts.push(plural(folders, "folder", "folders"));
    }
    parts.push(plural(total, "file", "files"));
    println!();
    println!(
        "  {}, {} in total",
        parts.join(", "),
        format_size(total_size)
    );
    if shown < total {
        println!("  showing the first {} — raise --limit to see more", shown);
    }
}

fn print_folder_contents(
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    target: u64,
    opts: &ListOptions,
) {
    let path = resolve_path(target, folder_name, folders);

    let mut child_folders: Vec<_> = folders
        .iter()
        .filter(|f| f.parentid == target && f.name != "/" && name_matches(&f.name, opts))
        .collect();
    sort_folders(&mut child_folders);
    let folder_sizes: Vec<_> = child_folders
        .iter()
        .map(|f| folder_totals(f.id, folders, media, folder_name))
        .collect();
    let (files, total, total_size) = select_files(
        media
            .iter()
            .filter(|m| shown_in_folder(m, target, folders, folder_name)),
        opts,
    );

    if opts.json {
        let folders_json: Vec<_> = child_folders
            .iter()
            .zip(&folder_sizes)
            .map(|(f, (n, size))| {
                serde_json::json!({ "id": f.id, "name": f.name, "files": n, "size": size })
            })
            .collect();
        let files_json: Vec<_> = files.iter().map(|m| file_json(m, None)).collect();
        print_json(&serde_json::json!({
            "path": path,
            "folders": folders_json,
            "files": files_json,
        }));
        return;
    }

    // Breadcrumb
    let show_path = if path == "/" {
        "/".to_string()
    } else {
        format!("{}/", path)
    };
    println!("📁 {}", show_path);
    println!();

    if files.is_empty() && child_folders.is_empty() {
        if opts.pattern.is_some() {
            println!("  (nothing matches)");
        } else {
            println!("  (empty folder)");
        }
        return;
    }

    // Size the ID column to the longest ID shown
    let id_width = child_folders
        .iter()
        .map(|f| f.id)
        .chain(files.iter().map(|m| m.id))
        .map(|id| id.to_string().len())
        .max()
        .unwrap_or(8);

    // Subfolders
    let name_width = child_folders
        .iter()
        .map(|f| f.name.chars().count() + 1)
        .max()
        .unwrap_or(0);
    for (f, &(n, size)) in child_folders.iter().zip(&folder_sizes) {
        println!(
            "  {:<id_width$}  📁 {:<name_width$}  {}, {}",
            f.id,
            format!("{}/", f.name),
            plural(n, "file", "files"),
            format_size(size),
        );
    }
    if !child_folders.is_empty() && !files.is_empty() {
        println!();
    }

    // Files
    for item in &files {
        let name = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
        if opts.long {
            let date = format_date(file_date(item));
            println!("  {:<id_width$}  {:>9}  {}  {}", item.id, size, date, name);
        } else {
            println!("  {:<id_width$}  {:>9}  {}", item.id, size, name);
        }
    }

    // The total size includes what's inside the subfolders
    let folders_size: u64 = folder_sizes.iter().map(|&(_, size)| size).sum();
    print_summary(
        child_folders.len(),
        files.len(),
        total,
        total_size + folders_size,
    );
}

fn print_flat_all(
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    folders: &[crate::api::FolderEntry],
    target: u64,
    opts: &ListOptions,
) {
    let ids = subtree_ids(target, folders);
    let (files, total, total_size) = select_files(
        media
            .iter()
            .filter(|m| in_subtree(m, &ids, folders, folder_name)),
        opts,
    );
    let full_path = |item: &crate::api::MediaItem| {
        let name = item.name.as_deref().unwrap_or("?");
        let path = item
            .folderid
            .filter(|fid| folder_name.contains_key(fid))
            .map(|fid| resolve_path(fid, folder_name, folders))
            .unwrap_or_else(|| "/".into());
        if path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", path, name)
        }
    };

    if opts.json {
        let files_json: Vec<_> = files
            .iter()
            .map(|m| file_json(m, Some(full_path(m))))
            .collect();
        print_json(&serde_json::Value::Array(files_json));
        return;
    }

    let id_width = files
        .iter()
        .map(|m| m.id.to_string().len())
        .max()
        .unwrap_or(2)
        .max(2);
    if opts.long {
        println!(
            "{:<id_width$}  {:>9}  {:<16}  PATH",
            "ID", "SIZE", "DATE (UTC)"
        );
        println!("{:<id_width$}  {:>9}  {:<16}  ──", "──", "──", "──");
    } else {
        println!("{:<id_width$}  {:>9}  PATH", "ID", "SIZE");
        println!("{:<id_width$}  {:>9}  ──", "──", "──");
    }

    for item in &files {
        let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
        if opts.long {
            let date = format_date(file_date(item));
            println!(
                "{:<id_width$}  {:>9}  {}  {}",
                item.id,
                size,
                date,
                full_path(item)
            );
        } else {
            println!("{:<id_width$}  {:>9}  {}", item.id, size, full_path(item));
        }
    }

    print_summary(0, files.len(), total, total_size);
}

fn print_tree(
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    current: u64,
    prefix: &str,
    is_last: bool,
    opts: &ListOptions,
) {
    let name = folder_name.get(&current).map(|s| s.as_str()).unwrap_or("?");

    // Root: just print "/"
    if name == "/" && prefix.is_empty() {
        println!("📁 /");
    } else {
        let connector = if is_last { "└── " } else { "├── " };
        println!("{}{}📁 {}/", prefix, connector, name);
    }

    let new_prefix = if prefix.is_empty() {
        "    ".to_string()
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    };

    // Files in this folder
    let (here, _, _) = select_files(
        media
            .iter()
            .filter(|m| shown_in_folder(m, current, folders, folder_name)),
        opts,
    );

    // Child folders (excluding root which has parentid=0)
    let mut children: Vec<_> = folders
        .iter()
        .filter(|f| f.parentid == current && f.name != "/" && f.parentid != 0)
        .collect();
    sort_folders(&mut children);

    let total = here.len() + children.len();
    let mut idx = 0;

    for item in &here {
        idx += 1;
        let last = idx == total;
        let conn = if last { "└── " } else { "├── " };
        let fname = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
        if opts.long {
            let date = format_date(file_date(item));
            println!("{}{}📄 {}  ({}, {})", new_prefix, conn, fname, size, date);
        } else {
            println!("{}{}📄 {}  ({})", new_prefix, conn, fname, size);
        }
    }

    for child in children.iter() {
        idx += 1;
        let last = idx == total;
        print_tree(
            folders,
            media,
            folder_name,
            child.id,
            &new_prefix,
            last,
            opts,
        );
    }
}

/// Resolve a user-supplied path string to a folder ID.
///
/// Supported formats:
/// - `None` → root folder
/// - `"12345678"` (numeric) → direct ID, or a root folder with that name
///   (e.g. `2024`) if no folder has that ID
/// - `"/Photos/2024"` (absolute path) → traverse tree
/// - `"app"` (single name) → search direct children of root
fn resolve_target(
    path: Option<String>,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
) -> Result<u64, O2Error> {
    let path = match path {
        Some(p) => p,
        None => {
            return Ok(folders
                .iter()
                .find(|f| f.name == "/")
                .map(|f| f.id)
                .unwrap_or(0));
        }
    };

    // Numeric → direct folder ID; otherwise fall through and treat it as
    // a folder name, so folders like `2024` stay reachable.
    if let Ok(id) = path.parse::<u64>() {
        if folder_name.contains_key(&id) {
            return Ok(id);
        }
    }

    // Absolute path → split and traverse
    let parts: Vec<&str> = if let Some(stripped) = path.strip_prefix('/') {
        stripped.split('/').filter(|s| !s.is_empty()).collect()
    } else {
        path.split('/').filter(|s| !s.is_empty()).collect()
    };

    if parts.is_empty() {
        return Ok(folders
            .iter()
            .find(|f| f.name == "/")
            .map(|f| f.id)
            .unwrap_or(0));
    }

    // Build parent→children index
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    for f in folders {
        children.entry(f.parentid).or_default().push(f.id);
    }

    // Find root
    let root = folders
        .iter()
        .find(|f| f.name == "/")
        .map(|f| f.id)
        .unwrap_or(0);

    // Traverse from root
    let mut current = root;
    for part in &parts {
        let kids = children.get(&current).ok_or_else(|| {
            O2Error::NotFound(format!(
                "Folder '{}' has no children",
                folder_name.get(&current).unwrap_or(&"?".into())
            ))
        })?;
        let found = kids.iter().find_map(|kid| {
            folder_name
                .get(kid)
                .and_then(|name| if name == *part { Some(*kid) } else { None })
        });
        match found {
            Some(id) => current = id,
            None => {
                // Try case-insensitive match
                let lower = part.to_lowercase();
                let fuzzy = kids.iter().find_map(|kid| {
                    folder_name.get(kid).and_then(|name| {
                        if name.to_lowercase() == lower {
                            Some(*kid)
                        } else {
                            None
                        }
                    })
                });
                match fuzzy {
                    Some(id) => current = id,
                    None => {
                        return Err(O2Error::NotFound(format!(
                            "Folder '{}' not found. Try `o2cloud ls` to browse from root.",
                            part
                        )));
                    }
                }
            }
        }
    }

    Ok(current)
}

fn resolve_path(
    folder_id: u64,
    folder_name: &HashMap<u64, String>,
    folders: &[crate::api::FolderEntry],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = folder_id;
    let mut seen = std::collections::HashSet::new();
    loop {
        if !seen.insert(cur) {
            break;
        }
        if let Some(name) = folder_name.get(&cur) {
            if name == "/" {
                break;
            }
            parts.push(name.clone());
        }
        cur = folders
            .iter()
            .find(|f| f.id == cur)
            .map(|f| f.parentid)
            .unwrap_or(0);
        if cur == 0 {
            break;
        }
    }
    parts.reverse();
    if parts.is_empty() {
        "/".into()
    } else {
        format!("/{}", parts.join("/"))
    }
}

// ------------------------------------------------------------------
// Upload
// ------------------------------------------------------------------

/// Largest file O2 Cloud takes in one upload: the server announces it as
/// `sapi.upload.max-size-in-mb` (4096) in `/sapi/system/information`.
/// Larger files go up in parts of `DEFAULT_PART_SIZE`, which leaves a
/// margin in case the limit counts the whole request, not just the file.
const MAX_UPLOAD_MB: u64 = 4096;
pub const DEFAULT_PART_SIZE: u64 = (MAX_UPLOAD_MB - 96) * 1024 * 1024;

/// Most parts a file is split into (`name.001` … `name.999`).
const MAX_PARTS: u64 = 999;

/// Options shared by `upload`, `upload-dir` and `upload-zip`.
pub struct UploadOptions {
    /// Don't upload a file (or part) the target folder already has with
    /// the same name and size, so an interrupted upload can be resumed.
    pub skip_existing: bool,
    /// How many files `upload-dir` sends at the same time.
    pub jobs: usize,
    /// Files larger than this go up in parts of (at most) this size,
    /// named `name.001`, `name.002`, … (the 7-Zip / HJSplit convention,
    /// so `cat name.0* > name` joins them too); `download name` joins
    /// them. At least 2.
    pub part_size: u64,
}

/// Files already in the cloud, by (folder ID, name): their sizes and IDs.
type ExistingFiles = HashMap<(u64, String), Vec<(u64, u64)>>;

fn existing_files(media: &[crate::api::MediaItem]) -> ExistingFiles {
    let mut existing: ExistingFiles = HashMap::new();
    for m in media {
        if let (Some(folder), Some(name)) = (m.folderid, m.name.clone()) {
            existing
                .entry((folder, name))
                .or_default()
                .push((m.size.unwrap_or(0), m.id));
        }
    }
    existing
}

/// The pieces a file of `size` bytes is uploaded as: `(name, offset,
/// length)`. One piece under the file's own name if it fits in
/// `part_size`, else `name.001`, `name.002`, …
///
/// The last part is always smaller than the others: that is how
/// `download` tells a complete set from one whose last parts are missing
/// (an upload cut short, or parts not listed yet). For a size that is an
/// exact multiple of `part_size`, the parts are made a little smaller.
fn upload_pieces(
    name: &str,
    size: u64,
    part_size: u64,
) -> Result<Vec<(String, u64, u64)>, O2Error> {
    if size <= part_size {
        return Ok(vec![(name.to_string(), 0, size)]);
    }
    let mut part = part_size;
    while size.is_multiple_of(part) && part > 2 {
        part -= 1;
    }
    let count = size.div_ceil(part);
    if count > MAX_PARTS {
        return Err(O2Error::Invalid(format!(
            "{} would need {} parts; the most is {}",
            name, count, MAX_PARTS
        )));
    }
    Ok((0..count)
        .map(|i| {
            let offset = i * part;
            let len = part.min(size - offset);
            (part_name(name, i as usize + 1), offset, len)
        })
        .collect())
}

/// Name of part `n` (1-based) of a split file.
fn part_name(name: &str, n: usize) -> String {
    format!("{}.{:03}", name, n)
}

/// The part number of `candidate` if it is a part of `base`
/// (`base.001` … `base.999`).
fn part_number(base: &str, candidate: &str) -> Option<usize> {
    candidate
        .strip_prefix(base)?
        .strip_prefix('.')
        .filter(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n > 0)
}

/// Check the parts of `name` already in `folder_id` before uploading
/// `pieces` there. Without `skip_existing`, any part there is refused:
/// mixing them with the new ones would join two different files. With
/// it, the parts there must be ones this file would upload (same number
/// and size), so the upload resumes.
fn check_existing_parts(
    name: &str,
    pieces: &[(String, u64, u64)],
    folder_id: u64,
    existing: &ExistingFiles,
    skip_existing: bool,
) -> Result<(), O2Error> {
    let mut there: Vec<(usize, &Vec<(u64, u64)>)> = existing
        .iter()
        .filter(|((folder, _), _)| *folder == folder_id)
        .filter_map(|((_, n), found)| part_number(name, n).map(|n| (n, found)))
        .collect();
    if there.is_empty() {
        return Ok(());
    }
    there.sort_by_key(|(n, _)| *n);
    if !skip_existing {
        return Err(O2Error::Invalid(format!(
            "this folder already has parts of a file named {} ({}): delete them \
             first, or add --skip-existing to resume uploading the same file",
            name,
            part_name(name, there[0].0)
        )));
    }
    for (n, found) in there {
        let fits = matches!(
            (pieces.get(n - 1), found.as_slice()),
            (Some((_, _, len)), [(size, _)]) if size == len
        );
        if !fits {
            return Err(O2Error::Invalid(format!(
                "the parts of {} already in this folder don't match this file \
                 ({} differs); delete them first",
                name,
                part_name(name, n)
            )));
        }
    }
    Ok(())
}

/// The result of uploading one local file.
struct Uploaded {
    /// The media ID of each piece (one, or one per part), including
    /// pieces skipped because they were already there.
    ids: Vec<u64>,
    /// How many pieces went up in parts (0 if the file wasn't split).
    parts: usize,
    /// How many pieces were skipped (already in the folder).
    skipped: usize,
}

/// Upload the local file `path` (`size` bytes) to `folder_id` as `name`,
/// in parts if it is larger than `opts.part_size`. `existing` must list
/// the folder's files if `opts.skip_existing` is set or the file is split
/// (see `needs_listing`): with `opts.skip_existing`, pieces already there
/// are skipped.
#[allow(clippy::too_many_arguments)]
async fn upload_local_file(
    client: &O2Client,
    auth: &AuthConfig,
    path: &Path,
    name: &str,
    size: u64,
    modified: Option<std::time::SystemTime>,
    folder_id: u64,
    existing: &ExistingFiles,
    opts: &UploadOptions,
) -> Result<Uploaded, O2Error> {
    let pieces = upload_pieces(name, size, opts.part_size)?;
    let split = pieces.len() > 1;
    if split {
        check_existing_parts(name, &pieces, folder_id, existing, opts.skip_existing)?;
        eprintln!(
            "  {} is larger than {}: uploading it in {} parts ({}.001 …)",
            name,
            format_size(opts.part_size),
            pieces.len(),
            name
        );
    }

    let date_str = system_time_to_compact_iso(modified.unwrap_or_else(std::time::SystemTime::now));
    let mut uploaded = Uploaded {
        ids: Vec::new(),
        parts: if split { pieces.len() } else { 0 },
        skipped: 0,
    };
    for (i, (piece_name, offset, len)) in pieces.iter().enumerate() {
        let already = opts
            .skip_existing
            .then(|| existing.get(&(folder_id, piece_name.clone())))
            .flatten()
            .and_then(|found| found.iter().find(|(s, _)| s == len));
        if let Some((_, id)) = already {
            eprintln!("  = {} is already there (id: {}), skipped", piece_name, id);
            uploaded.ids.push(*id);
            uploaded.skipped += 1;
            continue;
        }
        if split {
            eprintln!(
                "  → part {}/{}: {} ({})",
                i + 1,
                pieces.len(),
                piece_name,
                format_size(*len)
            );
        }
        let meta = UploadMeta {
            // A part on its own isn't a valid file of the original type.
            contenttype: if split {
                "application/octet-stream".to_string()
            } else {
                mime_type_from_name(name)
            },
            creationdate: date_str.clone(),
            folderid: folder_id,
            modificationdate: date_str.clone(),
            name: piece_name.clone(),
            size: *len,
        };
        // Metadata and contents in one request, streamed from disk
        match client.upload_file(&meta, path, *offset, auth).await {
            Ok(id) => uploaded.ids.push(id),
            // Say what is already up, so the upload can be resumed.
            Err(e) if split && !uploaded.ids.is_empty() => {
                let ids: Vec<String> = uploaded.ids.iter().map(u64::to_string).collect();
                return Err(O2Error::Api(format!(
                    "{} — part {}/{} of {} failed; parts 1–{} are in the cloud \
                     (ids: {}): run the same command with --skip-existing to resume",
                    e,
                    i + 1,
                    pieces.len(),
                    name,
                    i,
                    ids.join(", ")
                )));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(uploaded)
}

/// Whether uploading needs the cloud's file listing: to skip what is
/// already there, or to check for old parts before a split upload.
fn needs_listing(opts: &UploadOptions, largest_file: u64) -> bool {
    opts.skip_existing || largest_file > opts.part_size
}

/// The root folder's ID.
async fn root_folder_id(client: &O2Client, auth: &AuthConfig) -> Result<u64, O2Error> {
    let folders = client.get_all_folders(auth).await?;
    folders
        .iter()
        .find(|f| f.name == "/")
        .map(|f| f.id)
        .ok_or_else(|| O2Error::NotFound("Root folder not found".into()))
}

/// Upload a local file to O2 Cloud.
pub async fn upload_file(
    client: &O2Client,
    auth: &AuthConfig,
    local_path: &Path,
    folder_id: Option<u64>,
    opts: &UploadOptions,
    json: bool,
) -> Result<(), O2Error> {
    // Read file metadata
    let metadata = tokio::fs::metadata(local_path).await.map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("Cannot read file {}: {}", local_path.display(), e),
        ))
    })?;
    let size = metadata.len();
    let name = local_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| O2Error::Invalid("Invalid file name".into()))?
        .to_string();

    let folder_id = match folder_id {
        Some(id) => id,
        None => root_folder_id(client, auth).await?,
    };
    let existing = if needs_listing(opts, size) {
        existing_files(&client.get_all_media(auth).await?)
    } else {
        ExistingFiles::new()
    };

    eprintln!(
        "→ Uploading {} ({})...",
        local_path.display(),
        format_size(size)
    );
    let uploaded = upload_local_file(
        client,
        auth,
        local_path,
        &name,
        size,
        metadata.modified().ok(),
        folder_id,
        &existing,
        opts,
    )
    .await?;

    let all_skipped = uploaded.skipped == uploaded.ids.len();
    let done = if all_skipped {
        "Already uploaded"
    } else {
        "Uploaded successfully"
    };
    match uploaded.ids.as_slice() {
        [id] => eprintln!("✓ {} (id: {})", done, id),
        ids => {
            let ids: Vec<String> = ids.iter().map(u64::to_string).collect();
            eprintln!(
                "✓ {} in {} parts (ids: {})",
                done,
                ids.len(),
                ids.join(", ")
            );
        }
    }
    if json {
        let mut out = serde_json::json!({
            "name": name,
            "folderid": folder_id,
            "size": size,
            "skipped": all_skipped,
        });
        if uploaded.parts > 0 {
            out["parts"] = uploaded.ids.clone().into();
        } else {
            out["id"] = uploaded.ids[0].into();
        }
        print_json(&out);
    }
    Ok(())
}

// ------------------------------------------------------------------
// Recursive Directory Upload
// ------------------------------------------------------------------

/// Upload a local directory recursively.  The directory is recreated as a
/// folder of the same name inside the target (reusing it if it exists),
/// with matching subfolders, then all files are uploaded, `opts.jobs` at
/// a time.
pub async fn upload_dir(
    client: &O2Client,
    auth: &AuthConfig,
    local_dir: &Path,
    parent_folder_id: Option<u64>,
    opts: &UploadOptions,
    json: bool,
) -> Result<(), O2Error> {
    use futures_util::StreamExt;

    // Resolve target parent folder
    let root_parent = match parent_folder_id {
        Some(id) => id,
        None => root_folder_id(client, auth).await?,
    };

    // Fetch all existing folders to check for duplicates
    let existing_folders = client.get_all_folders(auth).await?;

    // Collect all files and directories to upload, skipping hidden and
    // system entries (see `is_skipped_name`).
    // The walk is synchronous filesystem work, so keep it off the runtime.
    let dir = local_dir.to_path_buf();
    let (entries, skipped_entries, total_size, largest_file) = spawn_blocking_io(move || {
        let mut entries: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
        let mut skipped_entries = 0u32;
        walk_dir(&dir, &mut entries, &mut skipped_entries)?;
        let largest_file = entries
            .iter()
            .filter(|(_, is_dir)| !is_dir)
            .filter_map(|(p, _)| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .max()
            .unwrap_or(0);
        Ok((entries, skipped_entries, dir_size(&dir), largest_file))
    })
    .await?;
    let existing = if needs_listing(opts, largest_file) {
        existing_files(&client.get_all_media(auth).await?)
    } else {
        ExistingFiles::new()
    };

    // Count files for progress
    let file_count = entries.iter().filter(|(_, is_dir)| !is_dir).count();
    eprintln!(
        "→ Uploading {} ({}) — {} files, {} dirs...",
        local_dir.display(),
        format_size(total_size),
        file_count,
        entries.iter().filter(|(_, is_dir)| *is_dir).count() - 1, // -1 for root
    );

    // Map: local dir path → o2 folder id. The uploaded directory itself
    // becomes a folder of the same name inside the target (like `cp -r`);
    // `canonicalize` gives `.`/`..` their real name.
    let mut folder_map: HashMap<PathBuf, u64> = HashMap::new();
    let top_name = local_dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let top_id = match top_name {
        Some(name) => ensure_folder(client, auth, &existing_folders, &name, root_parent).await?,
        // Only `/` has no name: upload its contents into the target.
        None => root_parent,
    };
    folder_map.insert(local_dir.to_path_buf(), top_id);

    // Create folders first (breadth-first: parent must exist before child)
    let mut dirs: Vec<_> = entries.iter().filter(|(_, is_dir)| *is_dir).collect();
    dirs.sort_by_key(|(p, _)| p.as_os_str().len()); // shortest path first = parent first

    for (dir_path, _) in &dirs {
        if *dir_path == local_dir {
            continue;
        }
        let parent_local = dir_path.parent().unwrap();
        let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(top_id);
        let name = dir_path.file_name().unwrap().to_string_lossy().into_owned();
        let folder_id = ensure_folder(client, auth, &existing_folders, &name, parent_o2).await?;
        folder_map.insert(dir_path.clone(), folder_id);
    }

    // Upload files, `opts.jobs` at a time.
    let files: Vec<(usize, &PathBuf, u64)> = entries
        .iter()
        .filter(|(_, is_dir)| !is_dir)
        .enumerate()
        .map(|(i, (file_path, _))| {
            let parent_local = file_path.parent().unwrap();
            let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(top_id);
            (i, file_path, parent_o2)
        })
        .collect();
    let existing = &existing;
    let mut results: Vec<(usize, serde_json::Value, Result<Uploaded, O2Error>)> =
        futures_util::stream::iter(files)
            .map(|(i, file_path, parent_o2)| async move {
                let name = file_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let file_meta = tokio::fs::metadata(file_path).await.ok();
                let size = file_meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = file_meta.and_then(|m| m.modified().ok());

                eprintln!(
                    "  [{}/{}] 📄 {} ({})",
                    i + 1,
                    file_count,
                    name,
                    format_size(size)
                );

                // Path inside the uploaded directory, with `/` on every platform.
                let local = file_path
                    .strip_prefix(local_dir)
                    .unwrap_or(file_path)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let result = serde_json::json!({
                    "local": local,
                    "name": name,
                    "folderid": parent_o2,
                    "size": size,
                });
                let uploaded = upload_local_file(
                    client, auth, file_path, &name, size, modified, parent_o2, existing, opts,
                )
                .await;
                if let Err(e) = &uploaded {
                    eprintln!("  ⚠ Upload failed for {}: {}", name, e);
                }
                (i, result, uploaded)
            })
            .buffer_unordered(opts.jobs.max(1))
            .collect()
            .await;
    results.sort_by_key(|(i, _, _)| *i);

    // One entry per file for `--json`, failed uploads included.
    let (mut succeeded, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    let results: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(_, mut result, uploaded)| {
            match uploaded {
                Ok(up) => {
                    if up.skipped == up.ids.len() {
                        skipped += 1;
                    } else {
                        succeeded += 1;
                    }
                    result["skipped"] = (up.skipped == up.ids.len()).into();
                    if up.parts > 0 {
                        result["parts"] = up.ids.into();
                    } else {
                        result["id"] = up.ids[0].into();
                    }
                }
                Err(e) => {
                    result["error"] = e.to_string().into();
                    failed += 1;
                }
            }
            result
        })
        .collect();

    if skipped_entries > 0 {
        eprintln!(
            "  (skipped {} hidden or system files/dirs: .git, .DS_Store, Thumbs.db, etc.)",
            skipped_entries
        );
    }
    if skipped > 0 {
        eprintln!("  ({} file(s) were already uploaded)", skipped);
    }
    if failed > 0 {
        eprintln!("⚠ {} file(s) failed to upload", failed);
    }
    eprintln!("✓ Uploaded {} files to {}", succeeded, local_dir.display());
    if json {
        print_json(&serde_json::Value::Array(results));
    }

    if failed > 0 {
        return Err(O2Error::Api(format!(
            "{} of {} files failed to upload",
            failed, file_count
        )));
    }
    Ok(())
}

/// Return the ID of the folder `name` inside `parent`, creating it if it
/// doesn't exist yet.
async fn ensure_folder(
    client: &O2Client,
    auth: &AuthConfig,
    existing_folders: &[crate::api::FolderEntry],
    name: &str,
    parent: u64,
) -> Result<u64, O2Error> {
    if let Some(f) = existing_folders
        .iter()
        .find(|f| f.name == name && f.parentid == parent)
    {
        eprintln!("  📁 {}/ (exists id:{})", name, f.id);
        return Ok(f.id);
    }
    let id = client.create_folder(name, parent, auth).await?;
    eprintln!("  📁 {}/ (created id:{})", name, id);
    Ok(id)
}

/// How a directory entry should be treated when walking a local tree.
enum EntryKind {
    Dir,
    File,
    /// A symlink pointing at a directory. Never followed: a link back to
    /// an ancestor would otherwise recurse forever.
    SymlinkedDir,
}

fn entry_kind(entry: &std::fs::DirEntry) -> EntryKind {
    // `DirEntry::file_type` does not follow symlinks, unlike `Path::is_dir`.
    match entry.file_type() {
        Ok(t) if t.is_dir() => EntryKind::Dir,
        Ok(t) if t.is_symlink() && entry.path().is_dir() => EntryKind::SymlinkedDir,
        _ => EntryKind::File,
    }
}

/// Files and folders the OS creates for its own use, which shouldn't
/// clutter the cloud. Compared in lowercase.
const SYSTEM_NAMES: &[&str] = &[
    "thumbs.db",
    "ehthumbs.db",
    "desktop.ini",
    "$recycle.bin",
    "system volume information",
    "icon\r", // macOS custom folder icon
];

/// Whether a walk skips the entry `name`: hidden entries (starting with
/// `.`: O2 Cloud rejects dotfiles, and dotdirs like `.git` should never
/// be uploaded) and the OS files in `SYSTEM_NAMES`.
fn is_skipped_name(name: &str) -> bool {
    name.starts_with('.') || SYSTEM_NAMES.contains(&name.to_lowercase().as_str())
}

/// Recursively collect files and directories under `dir`, skipping the
/// entries `is_skipped_name` rejects.
/// Symlinked files are followed; symlinked directories are skipped.
fn walk_dir(
    dir: &Path,
    entries: &mut Vec<(PathBuf, bool)>,
    skipped: &mut u32,
) -> Result<(), O2Error> {
    entries.push((dir.to_path_buf(), true));
    for entry in std::fs::read_dir(dir).map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("Cannot read dir {}: {}", dir.display(), e),
        ))
    })? {
        let entry = entry.map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Read error in {}: {}", dir.display(), e),
            ))
        })?;
        let path = entry.path();
        let skip = path
            .file_name()
            .is_some_and(|n| is_skipped_name(&n.to_string_lossy()));
        if skip {
            *skipped += 1;
            continue;
        }
        match entry_kind(&entry) {
            EntryKind::Dir => walk_dir(&path, entries, skipped)?,
            EntryKind::File => entries.push((path, false)),
            EntryKind::SymlinkedDir => {
                eprintln!("  ⚠ Skipping symlinked directory {}", path.display())
            }
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let skip = path
                .file_name()
                .is_some_and(|n| is_skipped_name(&n.to_string_lossy()));
            if skip {
                continue;
            }
            match entry_kind(&entry) {
                EntryKind::Dir => total += dir_size(&path),
                EntryKind::File => {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        total += meta.len();
                    }
                }
                EntryKind::SymlinkedDir => {}
            }
        }
    }
    total
}

// ------------------------------------------------------------------
// Zip & Upload
// ------------------------------------------------------------------

/// Create a zip of a local directory and upload it as a single file.
pub async fn upload_zip(
    client: &O2Client,
    auth: &AuthConfig,
    local_dir: &Path,
    folder_id: Option<u64>,
    opts: &UploadOptions,
    json: bool,
) -> Result<(), O2Error> {
    if !local_dir.is_dir() {
        return Err(O2Error::Invalid(format!(
            "Not a directory: {}",
            local_dir.display()
        )));
    }

    // `file_name()` is `None` for paths like `.` or `..`, so resolve first.
    let dir_name = local_dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "archive".to_string());
    let zip_name = format!("{}.zip", dir_name);

    // Build the zip inside a fresh private directory rather than directly
    // in the shared temp dir, so a pre-existing file or symlink planted at
    // a predictable path can't be clobbered or followed. The directory is
    // removed afterwards whether or not the upload succeeds.
    let tmp_dir = create_private_temp_dir()?;
    let tmp_zip = tmp_dir.join(&zip_name);
    let result = zip_and_upload(client, auth, local_dir, &tmp_zip, folder_id, opts, json).await;
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

/// Run synchronous filesystem work on tokio's blocking thread pool.
async fn spawn_blocking_io<T, F>(f: F) -> Result<T, O2Error>
where
    F: FnOnce() -> Result<T, O2Error> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(|e| {
        O2Error::Io(std::io::Error::other(format!(
            "Background task failed: {}",
            e
        )))
    })?
}

/// Create a new, uniquely named directory under the system temp dir that
/// only the current user can access.
fn create_private_temp_dir() -> Result<PathBuf, O2Error> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("o2cloud-zip-{}-{:08x}", std::process::id(), nanos));

    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    // `create` (non-recursive) fails if the path already exists.
    builder.create(&dir).map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("Cannot create temp dir {}: {}", dir.display(), e),
        ))
    })?;
    Ok(dir)
}

async fn zip_and_upload(
    client: &O2Client,
    auth: &AuthConfig,
    local_dir: &Path,
    tmp_zip: &Path,
    folder_id: Option<u64>,
    opts: &UploadOptions,
    json: bool,
) -> Result<(), O2Error> {
    eprintln!("→ Zipping {}...", local_dir.display());
    let (dir, zip_path) = (local_dir.to_path_buf(), tmp_zip.to_path_buf());
    let total_size = spawn_blocking_io(move || build_zip(&dir, &zip_path)).await?;
    eprintln!(
        "  Created {} ({})",
        tmp_zip.display(),
        format_size(total_size)
    );

    upload_file(client, auth, tmp_zip, folder_id, opts, json).await
}

/// Write a complete zip archive of `local_dir` to `zip_path`, returning
/// its size in bytes.
fn build_zip(local_dir: &Path, zip_path: &Path) -> Result<u64, O2Error> {
    let file = std::fs::File::create(zip_path).map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("Cannot create zip: {}", e),
        ))
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    add_dir_to_zip(&mut zip, local_dir, local_dir, options)?;
    // Write the central directory; without this the archive is unreadable.
    let file = zip
        .finish()
        .map_err(|e| O2Error::Io(std::io::Error::other(format!("Zip error: {}", e))))?;

    Ok(file.metadata().map(|m| m.len()).unwrap_or(0))
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
    options: zip::write::SimpleFileOptions,
) -> Result<(), O2Error> {
    for entry in std::fs::read_dir(dir).map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("Cannot read dir {}: {}", dir.display(), e),
        ))
    })? {
        let entry = entry.map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Read error in {}: {}", dir.display(), e),
            ))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap();

        match entry_kind(&entry) {
            EntryKind::Dir => add_dir_to_zip(zip, base, &path, options)?,
            EntryKind::SymlinkedDir => {
                eprintln!("  ⚠ Skipping symlinked directory {}", path.display())
            }
            EntryKind::File => {
                // ZIP entries always use `/` as the separator, regardless of
                // the host OS path separator (important on Windows).
                let entry_name = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                zip.start_file(&entry_name, options)
                    .map_err(|e| O2Error::Io(std::io::Error::other(format!("Zip error: {}", e))))?;
                let mut src = std::fs::File::open(&path).map_err(|e| {
                    O2Error::Io(std::io::Error::new(
                        e.kind(),
                        format!("Cannot read {}: {}", path.display(), e),
                    ))
                })?;
                std::io::copy(&mut src, zip).map_err(|e| {
                    O2Error::Io(std::io::Error::other(format!(
                        "Zip write error for {}: {}",
                        path.display(),
                        e
                    )))
                })?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------
// Download
// ------------------------------------------------------------------

/// Reduce a server-supplied media name to a bare file name that is safe to
/// create in the current directory: any directory components (`../`,
/// absolute paths, either slash style) are stripped. Returns `None` if
/// nothing usable remains.
fn safe_local_file_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next()?;
    match last {
        "" | "." | ".." => None,
        n => Some(n.to_string()),
    }
}

/// Download a file from O2 Cloud. `target` is a media ID, a file path
/// (`/docs/a.txt`) or a file name if it is unique, as for `rm`.
///
/// If `output` is `None`, the file is saved under its real name in the
/// current directory (falling back to `{id}.bin` if the name is empty or
/// unsafe).
/// Refuses to overwrite an existing local file.
pub async fn download_file(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    output: Option<PathBuf>,
    json: bool,
) -> Result<(), O2Error> {
    // Fetch the signed download URL (and real name) from the media listing
    let folders = client.get_all_folders(auth).await?;
    let media = media_for_target(client, auth, target, &folders).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();
    let by_id = target
        .parse::<u64>()
        .ok()
        .and_then(|id| media.iter().find(|m| m.id == id));
    // The file's name and the media items that hold it: one, or the
    // parts of a file uploaded in parts.
    let (name, pieces) = match by_id {
        Some(item) => (item.name.clone(), vec![item]),
        // Not a known ID: resolve as a file path or unique file name.
        None => match find_file(target, &folders, &folder_name, &media) {
            Ok(item) => (item.name.clone(), vec![item]),
            // No such file, but maybe it went up in parts.
            Err(O2Error::NotFound(msg)) => {
                let first = match find_file(&part_name(target, 1), &folders, &folder_name, &media) {
                    Ok(first) => first,
                    Err(O2Error::NotFound(_)) => return Err(O2Error::NotFound(msg)),
                    Err(e) => return Err(e),
                };
                let base = target.rsplit('/').next().unwrap_or(target);
                (Some(base.to_string()), file_parts(first, base, &media)?)
            }
            Err(e) => return Err(e),
        },
    };
    let ids: Vec<u64> = pieces.iter().map(|m| m.id).collect();

    let output_path = match output {
        Some(p) => p,
        None => match name.as_deref().and_then(safe_local_file_name) {
            Some(name) => PathBuf::from(name),
            None => PathBuf::from(format!("{}.bin", ids[0])),
        },
    };

    if output_path.exists() {
        return Err(O2Error::Invalid(format!(
            "Refusing to overwrite existing file: {}",
            output_path.display()
        )));
    }

    let mut file = crate::api::create_new_file(&output_path).await?;
    let mut bytes_written = 0;
    for (i, item) in pieces.iter().enumerate() {
        if pieces.len() > 1 {
            eprintln!(
                "→ Downloading part {}/{} (media {})...",
                i + 1,
                pieces.len(),
                item.id
            );
        } else {
            eprintln!("→ Downloading media {}...", item.id);
        }
        // The listing's signed URLs may expire during a long join, so
        // each part gets a fresh one.
        let fresh_url = pieces.len() > 1;
        let result = download_piece(client, auth, item, fresh_url, &mut file, &output_path).await;
        match result {
            Ok(n) => bytes_written += n,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&output_path).await;
                return Err(e);
            }
        }
    }

    eprintln!(
        "✓ Downloaded {} to {}",
        format_size(bytes_written),
        output_path.display()
    );
    if json {
        let path = output_path.canonicalize().unwrap_or(output_path);
        let mut out = serde_json::json!({
            "name": name,
            "path": path.display().to_string(),
            "size": bytes_written,
        });
        if ids.len() > 1 {
            out["parts"] = ids.into();
        } else {
            out["id"] = ids[0].into();
        }
        print_json(&out);
    }
    Ok(())
}

/// Download one media item and append it to `file`; with `fresh_url`, by
/// a download URL asked for now rather than the listing's.
async fn download_piece(
    client: &O2Client,
    auth: &AuthConfig,
    item: &crate::api::MediaItem,
    fresh_url: bool,
    file: &mut tokio::fs::File,
    path: &Path,
) -> Result<u64, O2Error> {
    // A file uploaded a moment ago is listed without a URL until the
    // server has processed it; ask for it by ID then, or when `fresh_url`.
    let download_url = match item.url.clone() {
        Some(url) if !fresh_url => url,
        _ => client.media_download_url(item.id, auth).await?,
    };
    client
        .download_into(&download_url, file, path, item.size)
        .await
}

/// The parts of the file `base` uploaded in parts, in order, starting
/// with its first part `first`: every `base.NNN` in the same folder.
/// Fails unless they make a complete file: no part missing between the
/// first and the last, every part but the last the same size, and the
/// last one smaller (`upload_pieces` makes sure of that, so a full-size
/// last part means later ones are missing). Parts can be missing because
/// an upload was cut short, or because the listing lags behind it.
fn file_parts<'a>(
    first: &'a crate::api::MediaItem,
    base: &str,
    media: &'a [crate::api::MediaItem],
) -> Result<Vec<&'a crate::api::MediaItem>, O2Error> {
    let mut by_number: HashMap<usize, Vec<&crate::api::MediaItem>> = HashMap::new();
    for m in media.iter().filter(|m| m.folderid == first.folderid) {
        if let Some(n) = m.name.as_deref().and_then(|n| part_number(base, n)) {
            by_number.entry(n).or_default().push(m);
        }
    }
    let incomplete = |why: String| {
        O2Error::NotFound(format!(
            "a complete set of parts of {}: {}. If they were uploaded a moment \
             ago, try again in a minute; otherwise download the parts by ID",
            base, why
        ))
    };
    let last = by_number.keys().copied().max().unwrap_or(1);
    let parts = (1..=last)
        .map(|n| match by_number.get(&n).map(Vec::as_slice) {
            Some([part]) => Ok(*part),
            Some(found) => Err(O2Error::Ambiguous(format!(
                "{} files are named '{}' in that folder; download the parts by ID",
                found.len(),
                part_name(base, n)
            ))),
            None => Err(incomplete(format!("{} is missing", part_name(base, n)))),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let full = parts[0].size;
    if let Some(odd) = parts[..parts.len() - 1].iter().find(|p| p.size != full) {
        return Err(incomplete(format!(
            "{} isn't the same size as the other parts",
            odd.name.as_deref().unwrap_or("?")
        )));
    }
    let last_part = parts[parts.len() - 1];
    let last_is_smaller = matches!((full, last_part.size), (Some(f), Some(l)) if l < f);
    if !last_is_smaller {
        return Err(incomplete(format!(
            "the parts after {} are missing",
            last_part.name.as_deref().unwrap_or("?")
        )));
    }
    Ok(parts)
}

// ------------------------------------------------------------------
// Find
// ------------------------------------------------------------------

/// Search files and folders by name (case-insensitive substring).
pub async fn find(
    client: &O2Client,
    auth: &AuthConfig,
    query: &str,
    json: bool,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = client.get_all_media(auth).await?;

    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let q = query.to_lowercase();
    let mut found = 0u32;
    let mut folders_json = Vec::new();
    let mut files_json = Vec::new();

    // Folders
    for f in &folders {
        if f.name.to_lowercase().contains(&q) {
            let path = resolve_path(f.id, &folder_name, &folders);
            if json {
                folders_json.push(serde_json::json!({ "id": f.id, "name": f.name, "path": path }));
            } else {
                println!("📁 {:<12}  {}/", f.id, path);
            }
            found += 1;
        }
    }

    // Files
    for item in &media {
        let name = item.name.as_deref().unwrap_or("");
        if name.to_lowercase().contains(&q) {
            let folder_path = item
                .folderid
                .map(|fid| resolve_path(fid, &folder_name, &folders))
                .unwrap_or_else(|| "/".into());
            let full = if folder_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", folder_path, name)
            };
            if json {
                files_json.push(file_json(item, Some(full)));
            } else {
                let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
                println!("📄 {:<12} {:>8}  {}", item.id, size, full);
            }
            found += 1;
        }
    }

    if json {
        print_json(&serde_json::json!({ "folders": folders_json, "files": files_json }));
    } else if found == 0 {
        println!("No results for '{}'", query);
    } else {
        println!("──\n{} results for '{}'", found, query);
    }

    Ok(())
}

// ------------------------------------------------------------------
// Delete
// ------------------------------------------------------------------

/// Delete a file or folder.  `target` can be a numeric ID, a folder path,
/// a file path (e.g. `/docs/a.txt`), or a bare file name if it is unique.
/// With `--recursive`, deletes a folder and all its contents; unless
/// `assume_yes` is set, this prompts for confirmation first.
pub async fn delete_target(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    recursive: bool,
    assume_yes: bool,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = media_for_target(client, auth, target, &folders).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    match resolve_item(target, &folders, &folder_name, &media)? {
        Item::Folder(id) => {
            delete_folder_checked(
                client,
                auth,
                id,
                &folders,
                &folder_name,
                &media,
                recursive,
                assume_yes,
            )
            .await
        }
        Item::File(item) => delete_file(client, auth, item).await,
    }
}

/// A file or folder named on the command line.
enum Item<'a> {
    Folder(u64),
    File(&'a crate::api::MediaItem),
}

/// Resolve `target` to a folder or a file. Tried in order: a numeric
/// folder ID, a numeric file ID, a folder path/name, then a file path or
/// unique file name (see `find_file`).
/// How long `media_for_target` waits for a new upload to show up, and
/// how often it asks.
const NEW_UPLOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
const NEW_UPLOAD_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// The account's files, for resolving `target`. The listing shows a new
/// upload only after 10–40 s, so a numeric `target` that is neither a
/// folder in `folders`, a listed file nor the name of one, but is newer
/// than every listed file (media IDs only grow), is asked for by ID until
/// it shows up: `rm`, `mv` & co. then work on the ID `upload` just printed.
async fn media_for_target(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    folders: &[crate::api::FolderEntry],
) -> Result<Vec<crate::api::MediaItem>, O2Error> {
    let mut media = client.get_all_media(auth).await?;
    let Ok(id) = target.parse::<u64>() else {
        return Ok(media);
    };
    let known = folders.iter().any(|f| f.id == id) || media.iter().any(|m| m.id == id);
    let newest = media.iter().map(|m| m.id).max().unwrap_or(0);
    if known || id <= newest {
        return Ok(media);
    }
    // A folder or file *named* like the number (e.g. `2024`) wins, as in
    // `resolve_item`, without waiting.
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();
    let is_name = resolve_target(Some(target.to_string()), folders, &folder_name).is_ok()
        || !matches!(
            find_file(target, folders, &folder_name, &media),
            Err(O2Error::NotFound(_))
        );
    if is_name {
        return Ok(media);
    }

    let start = std::time::Instant::now();
    loop {
        match client.get_media_by_id(id, auth).await {
            Ok(Some(item)) => {
                media.push(item);
                return Ok(media);
            }
            Ok(None) => {}
            // Leave it to the usual lookup to say it wasn't found.
            Err(_) => return Ok(media),
        }
        if start.elapsed() >= NEW_UPLOAD_WAIT {
            return Ok(media);
        }
        if start.elapsed() < NEW_UPLOAD_POLL {
            eprintln!(
                "  {} isn't listed yet; waiting up to {} s in case it was just uploaded...",
                id,
                NEW_UPLOAD_WAIT.as_secs()
            );
        }
        tokio::time::sleep(NEW_UPLOAD_POLL).await;
    }
}

fn resolve_item<'a>(
    target: &str,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
    media: &'a [crate::api::MediaItem],
) -> Result<Item<'a>, O2Error> {
    // Numeric ID: a folder or a file. If it matches neither, fall through
    // and treat it as a name (e.g. a folder called `2024`).
    if let Ok(id) = target.parse::<u64>() {
        if folder_name.contains_key(&id) {
            return Ok(Item::Folder(id));
        }
        if let Some(item) = media.iter().find(|m| m.id == id) {
            return Ok(Item::File(item));
        }
    }
    if let Ok(folder_id) = resolve_target(Some(target.to_string()), folders, folder_name) {
        return Ok(Item::Folder(folder_id));
    }
    find_file(target, folders, folder_name, media).map(Item::File)
}

/// Full cloud path of a file, e.g. `/docs/a.txt`.
fn file_path(
    item: &crate::api::MediaItem,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
) -> String {
    let dir = item
        .folderid
        .map(|fid| resolve_path(fid, folder_name, folders))
        .unwrap_or_else(|| "/".into());
    let name = item.name.as_deref().unwrap_or("?");
    if dir == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", dir, name)
    }
}

/// Error for operations the web client does differently for folders and
/// that the CLI doesn't support yet.
fn folder_unsupported(what: &str) -> O2Error {
    O2Error::Invalid(format!("{} folders isn't supported yet (only files)", what))
}

// ------------------------------------------------------------------
// Share
// ------------------------------------------------------------------

/// Create a public link to a file or folder and print its URL (to stdout,
/// alone, so it can be captured by scripts).
pub async fn share(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    json: bool,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = media_for_target(client, auth, target, &folders).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let (link, what, is_folder) = match resolve_item(target, &folders, &folder_name, &media)? {
        Item::Folder(id) => {
            if is_root_folder(id, &folders) {
                return Err(O2Error::Invalid(
                    "refusing to share the root folder (it would publish the whole account)".into(),
                ));
            }
            let path = resolve_path(id, &folder_name, &folders);
            (client.share_folder(id, auth).await?, path, true)
        }
        Item::File(item) => (
            client.share_files(&[item.id], auth).await?,
            file_path(item, &folders, &folder_name),
            false,
        ),
    };

    eprintln!(
        "✓ Public link created for {}{} — anyone with it can access it:",
        what,
        if is_folder { "/" } else { "" }
    );
    if json {
        print_json(&serde_json::json!({
            "id": link.id,
            "url": link.url,
            "path": what,
            "folder": is_folder,
        }));
    } else {
        println!("{}", link.url);
    }
    if is_folder {
        eprintln!(
            "  (link {}; remove it with `o2cloud unshare {}`)",
            link.id, link.id
        );
    }
    Ok(())
}

/// List public folder links.
pub async fn list_shares(client: &O2Client, auth: &AuthConfig, json: bool) -> Result<(), O2Error> {
    let links = client.list_links(auth).await?;
    if links.is_empty() && json {
        print_json(&serde_json::json!([]));
        return Ok(());
    }
    if links.is_empty() {
        println!("No shared folders.");
        return Ok(());
    }
    let folders = client.get_all_folders(auth).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let folder_of = |link: &crate::api::PublicLink| match link.folderid {
        Some(fid) if folder_name.contains_key(&fid) => {
            Some(resolve_path(fid, &folder_name, &folders))
        }
        _ => None,
    };

    if json {
        let links_json: Vec<_> = links
            .iter()
            .map(|link| {
                serde_json::json!({
                    "id": link.id,
                    "url": link.url,
                    "uses": link.usages,
                    "folderid": link.folderid,
                    "path": folder_of(link),
                    "foldername": link.foldername,
                })
            })
            .collect();
        print_json(&serde_json::Value::Array(links_json));
        return Ok(());
    }

    // The folder goes last: its length varies, the URLs' doesn't.
    println!("{:<10} {:>5}  {:<88}  FOLDER", "LINK ID", "USES", "URL");
    for link in &links {
        let folder = match folder_of(link) {
            Some(path) => format!("{}/", path),
            None => link.foldername.clone().unwrap_or_else(|| "?".into()),
        };
        let uses = link
            .usages
            .map(|u| u.to_string())
            .unwrap_or_else(|| "-".into());
        println!("{:<10} {:>5}  {:<88}  {}", link.id, uses, link.url, folder);
    }
    Ok(())
}

/// Remove the public link(s) of a folder, or a link by its ID (as shown
/// by `o2cloud shares`).
pub async fn unshare(client: &O2Client, auth: &AuthConfig, target: &str) -> Result<(), O2Error> {
    let links = client.list_links(auth).await?;

    // A link ID from `o2cloud shares`.
    if let Ok(id) = target.parse::<u64>() {
        if links.iter().any(|l| l.id == id) {
            client.delete_links(&[id], auth).await?;
            eprintln!("✓ Removed public link {}", id);
            return Ok(());
        }
    }

    let folders = client.get_all_folders(auth).await?;
    let media = client.get_all_media(auth).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    match resolve_item(target, &folders, &folder_name, &media)? {
        Item::Folder(id) => {
            let path = format!("{}/", resolve_path(id, &folder_name, &folders));
            let ids: Vec<u64> = links
                .iter()
                .filter(|l| l.folderid == Some(id))
                .map(|l| l.id)
                .collect();
            if ids.is_empty() {
                return Err(O2Error::NotFound(format!("{} is not shared", path)));
            }
            client.delete_links(&ids, auth).await?;
            eprintln!("✓ {} is no longer shared", path);
            Ok(())
        }
        Item::File(_) => Err(O2Error::Invalid(
            "unsharing files isn't supported yet; remove the link from the O2 Cloud website".into(),
        )),
    }
}

// ------------------------------------------------------------------
// Move & rename
// ------------------------------------------------------------------

/// Move a file or folder into another folder.
pub async fn move_item(
    client: &O2Client,
    auth: &AuthConfig,
    source: &str,
    destination: &str,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = media_for_target(client, auth, source, &folders).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let item = match resolve_item(source, &folders, &folder_name, &media)? {
        Item::File(item) => item,
        Item::Folder(id) => {
            return move_folder(client, auth, id, destination, &folders, &folder_name).await
        }
    };
    let dest_id = resolve_target(Some(destination.to_string()), &folders, &folder_name)?;
    let dest_path = resolve_path(dest_id, &folder_name, &folders);
    let from = file_path(item, &folders, &folder_name);

    if item.folderid == Some(dest_id) {
        eprintln!("{} is already in {}", from, dest_path);
        return Ok(());
    }
    if media
        .iter()
        .any(|m| m.folderid == Some(dest_id) && m.name == item.name)
    {
        return Err(O2Error::Invalid(format!(
            "{} already has a file named '{}'",
            dest_path,
            item.name.as_deref().unwrap_or("?")
        )));
    }

    client
        .update_file(
            item.id,
            item.name.as_deref().unwrap_or(""),
            serde_json::json!({ "folderid": dest_id }),
            auth,
        )
        .await?;
    eprintln!("✓ Moved {} → {}", from, dest_path);
    Ok(())
}

/// Move folder `folder_id` (and everything in it) into `destination`.
async fn move_folder(
    client: &O2Client,
    auth: &AuthConfig,
    folder_id: u64,
    destination: &str,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
) -> Result<(), O2Error> {
    if is_root_folder(folder_id, folders) {
        return Err(O2Error::Invalid("refusing to move the root folder".into()));
    }
    let folder = folders
        .iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| O2Error::NotFound(format!("folder {}", folder_id)))?;
    let dest_id = resolve_target(Some(destination.to_string()), folders, folder_name)?;
    let from = resolve_path(folder_id, folder_name, folders);
    let dest_path = resolve_path(dest_id, folder_name, folders);

    if folder.parentid == dest_id {
        eprintln!("{}/ is already in {}", from, dest_path);
        return Ok(());
    }
    if subtree_ids(folder_id, folders).contains(&dest_id) {
        return Err(O2Error::Invalid(format!(
            "can't move {}/ into itself ({})",
            from, dest_path
        )));
    }
    if folders
        .iter()
        .any(|f| f.parentid == dest_id && f.name == folder.name)
    {
        return Err(O2Error::Invalid(format!(
            "{} already has a folder named '{}'",
            dest_path, folder.name
        )));
    }

    client
        .update_folder(folder_id, &folder.name, dest_id, auth)
        .await?;
    eprintln!("✓ Moved {}/ → {}", from, dest_path);
    Ok(())
}

/// The extension of a file name (`None` if it has none). Case matters:
/// the server keeps `.JPG` when asked to rename to `.jpg`.
fn file_extension(name: &str) -> Option<&std::ffi::OsStr> {
    Path::new(name).extension()
}

/// Check a bare file or folder name given on the command line.
fn validate_name(name: &str) -> Result<&str, O2Error> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(O2Error::Invalid(format!(
            "invalid name '{}' (use `o2cloud mv` to move between folders)",
            name
        )));
    }
    Ok(name)
}

/// Rename a file or folder (the new name is a bare name, not a path).
pub async fn rename_item(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    new_name: &str,
) -> Result<(), O2Error> {
    let new_name = validate_name(new_name)?;

    let folders = client.get_all_folders(auth).await?;
    let media = media_for_target(client, auth, target, &folders).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let item = match resolve_item(target, &folders, &folder_name, &media)? {
        Item::File(item) => item,
        Item::Folder(id) => {
            return rename_folder(client, auth, id, new_name, &folders, &folder_name).await
        }
    };
    let from = file_path(item, &folders, &folder_name);
    // The server keeps a file's extension: it accepts a rename to
    // `notes.txt` for `notes.pdf` but stores `notes.pdf`.
    let current_name = item.name.as_deref().unwrap_or("");
    if file_extension(current_name) != file_extension(new_name) {
        return Err(O2Error::Invalid(format!(
            "O2 Cloud can't change a file's extension ('{}' → '{}'); \
             download it and upload it under the new name instead",
            current_name, new_name
        )));
    }
    if media.iter().any(|m| {
        m.id != item.id && m.folderid == item.folderid && m.name.as_deref() == Some(new_name)
    }) {
        return Err(O2Error::Invalid(format!(
            "a file named '{}' already exists there",
            new_name
        )));
    }

    client
        .update_file(
            item.id,
            item.name.as_deref().unwrap_or(""),
            serde_json::json!({ "name": new_name }),
            auth,
        )
        .await?;
    eprintln!("✓ Renamed {} → {}", from, new_name);
    Ok(())
}

async fn rename_folder(
    client: &O2Client,
    auth: &AuthConfig,
    folder_id: u64,
    new_name: &str,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
) -> Result<(), O2Error> {
    if is_root_folder(folder_id, folders) {
        return Err(O2Error::Invalid(
            "refusing to rename the root folder".into(),
        ));
    }
    let folder = folders
        .iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| O2Error::NotFound(format!("folder {}", folder_id)))?;
    let from = resolve_path(folder_id, folder_name, folders);
    if folder.name == new_name {
        eprintln!("{}/ is already called '{}'", from, new_name);
        return Ok(());
    }
    if folders
        .iter()
        .any(|f| f.id != folder_id && f.parentid == folder.parentid && f.name == new_name)
    {
        return Err(O2Error::Invalid(format!(
            "a folder named '{}' already exists there",
            new_name
        )));
    }

    client
        .update_folder(folder_id, new_name, folder.parentid, auth)
        .await?;
    eprintln!("✓ Renamed {}/ → {}/", from, new_name);
    Ok(())
}

// ------------------------------------------------------------------
// Copy / mkdir / quota
// ------------------------------------------------------------------

/// Copy a file into a folder on the server (nothing is downloaded),
/// optionally under a new name.
pub async fn copy_item(
    client: &O2Client,
    auth: &AuthConfig,
    source: &str,
    destination: &str,
    new_name: Option<&str>,
    json: bool,
) -> Result<(), O2Error> {
    let new_name = new_name.map(validate_name).transpose()?;

    let folders = client.get_all_folders(auth).await?;
    let media = media_for_target(client, auth, source, &folders).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let item = match resolve_item(source, &folders, &folder_name, &media)? {
        Item::File(item) => item,
        Item::Folder(_) => return Err(folder_unsupported("Copying")),
    };
    let dest_id = resolve_target(Some(destination.to_string()), &folders, &folder_name)?;
    let dest_path = resolve_path(dest_id, &folder_name, &folders);
    let from = file_path(item, &folders, &folder_name);
    let name = new_name.or(item.name.as_deref()).unwrap_or("?");

    if media
        .iter()
        .any(|m| m.folderid == Some(dest_id) && m.name.as_deref() == Some(name))
    {
        return Err(O2Error::Invalid(format!(
            "{} already has a file named '{}' (use --name to give the copy another name)",
            dest_path, name
        )));
    }

    let id = client.copy_file(item.id, dest_id, new_name, auth).await?;
    let to = if dest_path == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", dest_path, name)
    };
    eprintln!("✓ Copied {} → {} (id:{})", from, to, id);
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "name": name,
            "folderid": dest_id,
            "path": to,
            "source": item.id,
        }));
    }
    Ok(())
}

/// Create a folder. `path` is absolute (`/a/b`) or relative to the root
/// (`a/b`). With `parents`, missing parent folders are created too and an
/// existing folder isn't an error, as with `mkdir -p`.
pub async fn make_folder(
    client: &O2Client,
    auth: &AuthConfig,
    path: &str,
    parents: bool,
    json: bool,
) -> Result<(), O2Error> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(O2Error::Invalid("the root folder already exists".into()));
    }
    for part in &parts {
        if *part == "." || *part == ".." {
            return Err(O2Error::Invalid(format!("invalid folder name '{}'", part)));
        }
    }

    let folders = client.get_all_folders(auth).await?;
    let mut current = folders
        .iter()
        .find(|f| f.name == "/")
        .map(|f| f.id)
        .ok_or_else(|| O2Error::NotFound("the root folder".into()))?;
    let mut shown = String::new();

    for (i, part) in parts.iter().enumerate() {
        shown.push('/');
        shown.push_str(part);
        let last = i == parts.len() - 1;
        // Exact name match only: the server treats `Docs` and `docs` as
        // different folders
        match folders
            .iter()
            .find(|f| f.parentid == current && f.name == *part)
        {
            Some(f) if last && !parents => {
                return Err(O2Error::Invalid(format!(
                    "{}/ already exists (id:{})",
                    shown, f.id
                )));
            }
            Some(f) => current = f.id,
            None if !last && !parents => {
                return Err(O2Error::NotFound(format!(
                    "folder {}/ (use -p to create missing parents)",
                    shown
                )));
            }
            None => {
                current = client.create_folder(part, current, auth).await?;
                eprintln!("✓ Created folder {}/ (id:{})", shown, current);
            }
        }
    }
    let created = !folders.iter().any(|f| f.id == current);
    if !created {
        eprintln!("{}/ already exists (id:{})", shown, current);
    }
    if json {
        print_json(&serde_json::json!({
            "id": current,
            "path": shown,
            "created": created,
        }));
    }
    Ok(())
}

/// Print the storage quota and usage.
pub async fn show_quota(client: &O2Client, auth: &AuthConfig, json: bool) -> Result<(), O2Error> {
    let space = client.storage_space(auth).await?;
    if json {
        print_json(&serde_json::json!(space));
        return Ok(());
    }
    if space.nolimit {
        println!("Used:   {} (no storage limit)", format_size(space.used));
    } else {
        let pct = if space.quota > 0 {
            space.used as f64 * 100.0 / space.quota as f64
        } else {
            0.0
        };
        println!(
            "Used:   {} of {} ({:.1}%)",
            format_size(space.used),
            format_size(space.quota),
            pct
        );
        println!("Free:   {}", format_size(space.free));
    }
    println!("Trash:  {}", format_size(space.softdeleted));
    Ok(())
}

/// Find exactly one file by path (`/docs/a.txt`) or by bare name
/// (`a.txt`, searched across all folders). Errors if nothing matches, or
/// if a bare name matches several files — deleting an arbitrary one of
/// them would be a guess.
fn find_file<'a>(
    target: &str,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
    media: &'a [crate::api::MediaItem],
) -> Result<&'a crate::api::MediaItem, O2Error> {
    let not_found = || {
        O2Error::NotFound(format!(
            "'{}'. Use a numeric ID, a folder path, or a file path.",
            target
        ))
    };

    let matches: Vec<&crate::api::MediaItem> = match target.rsplit_once('/') {
        Some((dir, name)) => {
            let dir = if dir.is_empty() { "/" } else { dir };
            let folder_id = resolve_target(Some(dir.to_string()), folders, folder_name)
                .map_err(|_| not_found())?;
            media
                .iter()
                .filter(|m| m.folderid == Some(folder_id) && m.name.as_deref() == Some(name))
                .collect()
        }
        None => media
            .iter()
            .filter(|m| m.name.as_deref() == Some(target))
            .collect(),
    };

    match matches.as_slice() {
        [] => Err(not_found()),
        [item] => Ok(item),
        many => {
            let listing: Vec<String> = many
                .iter()
                .map(|m| format!("  {:<12}  {}", m.id, file_path(m, folders, folder_name)))
                .collect();
            Err(O2Error::Ambiguous(format!(
                "'{}' is ambiguous — {} files have that name:\n{}\nUse the ID or the full path.",
                target,
                many.len(),
                listing.join("\n")
            )))
        }
    }
}

/// The account's root folder (`/`). Deleting it would trash everything.
fn is_root_folder(folder_id: u64, folders: &[crate::api::FolderEntry]) -> bool {
    folders.iter().any(|f| f.id == folder_id && f.name == "/")
}

/// Guard, confirm, then delete a folder.
#[allow(clippy::too_many_arguments)]
async fn delete_folder_checked(
    client: &O2Client,
    auth: &AuthConfig,
    folder_id: u64,
    folders: &[crate::api::FolderEntry],
    folder_name: &HashMap<u64, String>,
    media: &[crate::api::MediaItem],
    recursive: bool,
    assume_yes: bool,
) -> Result<(), O2Error> {
    if is_root_folder(folder_id, folders) {
        return Err(O2Error::Invalid(
            "refusing to delete the root folder".into(),
        ));
    }
    let path = resolve_path(folder_id, folder_name, folders);
    if !confirm_recursive_delete(folder_id, &path, folders, media, recursive, assume_yes)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    delete_folder(client, auth, folder_id, folders, media, recursive).await
}

/// Count all files and subfolders nested (at any depth) under `folder_id`.
fn count_recursive(
    folder_id: u64,
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
) -> (usize, usize) {
    let mut files = media
        .iter()
        .filter(|m| m.folderid == Some(folder_id))
        .count();
    let mut dirs = 0usize;
    for child in folders.iter().filter(|f| f.parentid == folder_id) {
        dirs += 1;
        let (child_files, child_dirs) = count_recursive(child.id, folders, media);
        files += child_files;
        dirs += child_dirs;
    }
    (files, dirs)
}

/// For a recursive delete with non-empty contents, ask the user to
/// confirm before anything is destroyed. Returns `Ok(true)` if the
/// delete should proceed. Non-recursive deletes and empty folders
/// always proceed without prompting.
fn confirm_recursive_delete(
    folder_id: u64,
    path: &str,
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    recursive: bool,
    assume_yes: bool,
) -> Result<bool, O2Error> {
    if !recursive || assume_yes {
        return Ok(true);
    }
    let (files, dirs) = count_recursive(folder_id, folders, media);
    if files == 0 && dirs == 0 {
        return Ok(true);
    }

    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(O2Error::Invalid(format!(
            "refusing to delete {}/ ({} file(s), {} subfolder(s)) without a TTY; \
                 re-run with --yes (-y) to confirm non-interactively",
            path, files, dirs
        )));
    }

    eprint!(
        "⚠ This will move {}/ and its {} file(s) and {} subfolder(s) to the trash. Continue? [y/N] ",
        path, files, dirs
    );
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(O2Error::Io)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

async fn delete_folder(
    client: &O2Client,
    auth: &AuthConfig,
    folder_id: u64,
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    recursive: bool,
) -> Result<(), O2Error> {
    let name = folders
        .iter()
        .find(|f| f.id == folder_id)
        .map(|f| f.name.as_str())
        .unwrap_or("?");

    // Count contents
    let file_count = media
        .iter()
        .filter(|m| m.folderid == Some(folder_id))
        .count();
    let child_dirs: Vec<_> = folders.iter().filter(|f| f.parentid == folder_id).collect();

    if !recursive && (!child_dirs.is_empty() || file_count > 0) {
        return Err(O2Error::Invalid(format!(
            "{}/ contains {} files and {} subfolders; use --recursive (-r) \
                 to delete the folder and all its contents",
            name,
            file_count,
            child_dirs.len()
        )));
    }

    // Recursively delete child folders
    for child in &child_dirs {
        Box::pin(delete_folder(client, auth, child.id, folders, media, true)).await?;
    }

    let files: Vec<u64> = media
        .iter()
        .filter(|m| m.folderid == Some(folder_id))
        .map(|m| m.id)
        .collect();
    delete_files_batched(client, auth, &files).await?;

    // Delete the folder itself
    client.delete_folder(folder_id, auth).await?;
    eprintln!("✓ Deleted folder {}/ (id:{})", name, folder_id);
    Ok(())
}

/// Whether `err` is a SAPI error with the given code (e.g. `MED-1025`).
fn is_sapi_code(err: &O2Error, code: &str) -> bool {
    matches!(err, O2Error::Api(msg) if msg.contains(&format!("({})", code)))
}

/// Soft-delete files in batches. The server caps the number of IDs per
/// request but doesn't say what the cap is: a batch over it fails with
/// MED-1025, so halve the batch and retry.
async fn delete_files_batched(
    client: &O2Client,
    auth: &AuthConfig,
    ids: &[u64],
) -> Result<(), O2Error> {
    let mut batch = 1000;
    let mut rest = ids;
    while !rest.is_empty() {
        let chunk = &rest[..batch.min(rest.len())];
        match client.delete_files(chunk, auth).await {
            Ok(()) => {}
            Err(e) if is_sapi_code(&e, "MED-1025") && chunk.len() > 1 => {
                batch = chunk.len() / 2;
                continue;
            }
            Err(e) if is_sapi_code(&e, "MED-1022") => {
                delete_files_skipping_trashed(client, auth, chunk).await?
            }
            Err(e) => return Err(e),
        }
        rest = &rest[chunk.len()..];
    }
    Ok(())
}

/// Delete a batch the server rejected with MED-1022 ("File already soft
/// deleted"): one trashed file fails the whole request, so split the
/// batch until the trashed files are isolated, and skip them.
async fn delete_files_skipping_trashed(
    client: &O2Client,
    auth: &AuthConfig,
    ids: &[u64],
) -> Result<(), O2Error> {
    if ids.len() <= 1 {
        return Ok(());
    }
    let (left, right) = ids.split_at(ids.len() / 2);
    for half in [left, right] {
        match client.delete_files(half, auth).await {
            Ok(()) => {}
            Err(e) if is_sapi_code(&e, "MED-1022") => {
                Box::pin(delete_files_skipping_trashed(client, auth, half)).await?
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Soft-delete a file by media ID (move to trash).
async fn delete_file(
    client: &O2Client,
    auth: &AuthConfig,
    item: &crate::api::MediaItem,
) -> Result<(), O2Error> {
    let media_id = item.id;
    let name = item.name.as_deref().unwrap_or("?");

    eprintln!("→ Deleting {} ({})...", name, media_id);
    client.delete_files(&[media_id], auth).await?;
    eprintln!("✓ Deleted {} ({})", name, media_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_zip_produces_readable_archive() {
        let tmp = std::env::temp_dir().join(format!("o2cli_test_zip_{}", std::process::id()));
        let src = tmp.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), b"world").unwrap();

        let zip_path = tmp.join("out.zip");
        let size = build_zip(&src, &zip_path).unwrap();
        assert_eq!(size, std::fs::metadata(&zip_path).unwrap().len());

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = archive
            .file_names()
            .map(|n| n.unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "sub/b.txt"]);

        let mut contents = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("sub/b.txt").unwrap(), &mut contents)
            .unwrap();
        assert_eq!(contents, "world");

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_safe_local_file_name() {
        assert_eq!(
            safe_local_file_name("photo.jpg").as_deref(),
            Some("photo.jpg")
        );
        assert_eq!(
            safe_local_file_name("../../.bashrc").as_deref(),
            Some(".bashrc")
        );
        assert_eq!(
            safe_local_file_name("/etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            safe_local_file_name("..\\evil.exe").as_deref(),
            Some("evil.exe")
        );
        assert_eq!(safe_local_file_name(""), None);
        assert_eq!(safe_local_file_name(".."), None);
        assert_eq!(safe_local_file_name("dir/"), None);
    }

    #[test]
    fn test_resolve_path() {
        let folders = vec![
            crate::api::FolderEntry {
                id: 1,
                name: "/".into(),
                parentid: 0,
            },
            crate::api::FolderEntry {
                id: 2,
                name: "docs".into(),
                parentid: 1,
            },
            crate::api::FolderEntry {
                id: 3,
                name: "images".into(),
                parentid: 2,
            },
        ];
        let folder_name: std::collections::HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();

        assert_eq!(resolve_path(1, &folder_name, &folders), "/");
        assert_eq!(resolve_path(2, &folder_name, &folders), "/docs");
        assert_eq!(resolve_path(3, &folder_name, &folders), "/docs/images");
    }

    #[test]
    fn test_resolve_target_by_id() {
        let folders = vec![
            crate::api::FolderEntry {
                id: 1,
                name: "/".into(),
                parentid: 0,
            },
            crate::api::FolderEntry {
                id: 42,
                name: "stuff".into(),
                parentid: 1,
            },
        ];
        let folder_name: std::collections::HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();

        assert_eq!(
            resolve_target(Some("42".into()), &folders, &folder_name).unwrap(),
            42
        );
        assert_eq!(resolve_target(None, &folders, &folder_name).unwrap(), 1);
    }

    #[test]
    fn test_resolve_target_by_id_not_found() {
        let folders = vec![
            crate::api::FolderEntry {
                id: 1,
                name: "/".into(),
                parentid: 0,
            },
            crate::api::FolderEntry {
                id: 42,
                name: "stuff".into(),
                parentid: 1,
            },
        ];
        let folder_name: std::collections::HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();

        assert!(resolve_target(Some("999".into()), &folders, &folder_name).is_err());
    }

    #[test]
    fn test_resolve_target_by_path() {
        let folders = vec![
            crate::api::FolderEntry {
                id: 1,
                name: "/".into(),
                parentid: 0,
            },
            crate::api::FolderEntry {
                id: 2,
                name: "docs".into(),
                parentid: 1,
            },
            crate::api::FolderEntry {
                id: 3,
                name: "images".into(),
                parentid: 2,
            },
        ];
        let folder_name: std::collections::HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();

        assert_eq!(
            resolve_target(Some("/docs".into()), &folders, &folder_name).unwrap(),
            2
        );
        assert_eq!(
            resolve_target(Some("/docs/images".into()), &folders, &folder_name).unwrap(),
            3
        );
    }

    #[test]
    fn test_resolve_target_not_found() {
        let folders = vec![crate::api::FolderEntry {
            id: 1,
            name: "/".into(),
            parentid: 0,
        }];
        let folder_name = std::collections::HashMap::from([(1u64, "/".into())]);

        assert!(resolve_target(Some("/nope".into()), &folders, &folder_name).is_err());
    }

    fn sample_tree() -> (
        Vec<crate::api::FolderEntry>,
        HashMap<u64, String>,
        Vec<crate::api::MediaItem>,
    ) {
        let folders = vec![
            crate::api::FolderEntry {
                id: 1,
                name: "/".into(),
                parentid: 0,
            },
            crate::api::FolderEntry {
                id: 2,
                name: "docs".into(),
                parentid: 1,
            },
            crate::api::FolderEntry {
                id: 3,
                name: "2024".into(),
                parentid: 1,
            },
        ];
        let folder_name = folders.iter().map(|f| (f.id, f.name.clone())).collect();
        let media: Vec<crate::api::MediaItem> = serde_json::from_str(
            r#"[
                {"id": 100, "name": "a.txt", "folderid": 2},
                {"id": 101, "name": "dup.txt", "folderid": 2},
                {"id": 102, "name": "dup.txt", "folderid": 3},
                {"id": 103, "name": "root.txt", "folderid": 1}
            ]"#,
        )
        .unwrap();
        (folders, folder_name, media)
    }

    #[test]
    fn test_resolve_target_numeric_name_fallback() {
        let (folders, folder_name, _) = sample_tree();
        // "2024" is not a folder ID, but it is a folder name under root.
        assert_eq!(
            resolve_target(Some("2024".into()), &folders, &folder_name).unwrap(),
            3
        );
        // A real ID still wins.
        assert_eq!(
            resolve_target(Some("2".into()), &folders, &folder_name).unwrap(),
            2
        );
    }

    #[test]
    fn test_find_file() {
        let (folders, folder_name, media) = sample_tree();
        let find = |t: &str| find_file(t, &folders, &folder_name, &media).map(|m| m.id);

        assert_eq!(find("a.txt").unwrap(), 100);
        assert_eq!(find("/docs/a.txt").unwrap(), 100);
        assert_eq!(find("/root.txt").unwrap(), 103);
        assert_eq!(find("/2024/dup.txt").unwrap(), 102);
        // Same bare name in two folders: refuse to guess.
        let err = find("dup.txt").unwrap_err().to_string();
        assert!(err.starts_with("'dup.txt' is ambiguous"), "{err}");
        assert!(err.contains("101           /docs/dup.txt"), "{err}");
        assert!(err.contains("102           /2024/dup.txt"), "{err}");
        assert!(find("missing.txt").is_err());
        assert!(find("/nope/a.txt").is_err());
        assert!(find("/2024/a.txt").is_err());
    }

    #[test]
    fn test_shown_in_folder_puts_orphans_in_root() {
        let (folders, folder_name, _) = sample_tree();
        let media: Vec<crate::api::MediaItem> = serde_json::from_str(
            r#"[
                {"id": 1, "name": "in-docs", "folderid": 2},
                {"id": 2, "name": "no-folder"},
                {"id": 3, "name": "unknown-folder", "folderid": 999}
            ]"#,
        )
        .unwrap();
        let shown = |folder: u64| -> Vec<u64> {
            media
                .iter()
                .filter(|m| shown_in_folder(m, folder, &folders, &folder_name))
                .map(|m| m.id)
                .collect()
        };
        assert_eq!(shown(2), vec![1]);
        assert_eq!(shown(1), vec![2, 3]);
        assert_eq!(shown(3), Vec::<u64>::new());
    }

    #[test]
    fn test_resolve_item() {
        let (folders, folder_name, media) = sample_tree();
        let kind = |t: &str| match resolve_item(t, &folders, &folder_name, &media) {
            Ok(Item::Folder(id)) => format!("folder {id}"),
            Ok(Item::File(m)) => format!("file {}", m.id),
            Err(_) => "error".into(),
        };
        assert_eq!(kind("2"), "folder 2"); // folder ID
        assert_eq!(kind("103"), "file 103"); // file ID
        assert_eq!(kind("/docs"), "folder 2");
        assert_eq!(kind("2024"), "folder 3"); // numeric name, not an ID
        assert_eq!(kind("/docs/a.txt"), "file 100");
        assert_eq!(kind("root.txt"), "file 103");
        assert_eq!(kind("dup.txt"), "error"); // ambiguous
        assert_eq!(kind("/nope"), "error");
    }

    #[test]
    fn test_is_root_folder() {
        let (folders, _, _) = sample_tree();
        assert!(is_root_folder(1, &folders));
        assert!(!is_root_folder(2, &folders));
        assert!(!is_root_folder(999, &folders));
    }

    #[test]
    fn test_count_recursive() {
        let folders = vec![
            crate::api::FolderEntry {
                id: 1,
                name: "/".into(),
                parentid: 0,
            },
            crate::api::FolderEntry {
                id: 2,
                name: "docs".into(),
                parentid: 1,
            },
            crate::api::FolderEntry {
                id: 3,
                name: "images".into(),
                parentid: 2,
            },
        ];
        let media: Vec<crate::api::MediaItem> = serde_json::from_str(
            r#"[
                {"id": 100, "name": "a.txt", "folderid": 2, "size": 1},
                {"id": 101, "name": "b.txt", "folderid": 3, "size": 2},
                {"id": 102, "name": "c.txt", "folderid": 3, "size": 3}
            ]"#,
        )
        .unwrap();

        // Folder 2 ("docs") contains 1 direct file + 1 subfolder ("images"),
        // which itself contains 2 files.
        assert_eq!(count_recursive(2, &folders, &media), (3, 1));
        // Folder 3 ("images") has no subfolders, just 2 files.
        assert_eq!(count_recursive(3, &folders, &media), (2, 0));
        // Root has no direct files, only the "docs" subtree.
        assert_eq!(count_recursive(1, &folders, &media), (3, 2));
    }

    #[cfg(unix)]
    #[test]
    fn test_walk_dir_and_zip_skip_symlinked_dir_loops() {
        let tmp = std::env::temp_dir().join(format!("o2cli_test_symlink_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub").join("f.txt"), b"x").unwrap();
        std::fs::write(tmp.join("outside.txt"), b"linked").unwrap();
        // Directory link back to an ancestor (would recurse forever) and a
        // file link (should be followed).
        std::os::unix::fs::symlink(&src, src.join("sub").join("loop")).unwrap();
        std::os::unix::fs::symlink(tmp.join("outside.txt"), src.join("link.txt")).unwrap();

        let mut entries: Vec<(PathBuf, bool)> = Vec::new();
        let mut skipped = 0u32;
        walk_dir(&src, &mut entries, &mut skipped).unwrap();
        let mut files: Vec<_> = entries
            .iter()
            .filter(|(_, is_dir)| !is_dir)
            .map(|(p, _)| p.strip_prefix(&src).unwrap().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, vec!["link.txt", "sub/f.txt"]);
        assert_eq!(dir_size(&src), 7);

        let zip_path = tmp.join("out.zip");
        build_zip(&src, &zip_path).unwrap();
        let archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        assert_eq!(archive.len(), 2);

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_walk_dir_skips_hidden_files_and_dirs() {
        let tmp = std::env::temp_dir().join(format!("o2cli_test_walk_dir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".hidden")).unwrap();
        std::fs::write(tmp.join(".hidden").join("inner.txt"), b"secret").unwrap();
        std::fs::write(tmp.join("visible.txt"), b"public").unwrap();
        std::fs::write(tmp.join(".dotfile"), b"nope").unwrap();

        let mut entries: Vec<(PathBuf, bool)> = Vec::new();
        let mut skipped = 0u32;
        walk_dir(&tmp, &mut entries, &mut skipped).unwrap();

        let files: Vec<_> = entries
            .iter()
            .filter(|(_, is_dir)| !is_dir)
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(files, vec!["visible.txt".to_string()]);
        assert_eq!(skipped, 2); // .hidden/ dir and .dotfile

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_walk_dir_skips_system_files() {
        let tmp = std::env::temp_dir().join(format!("o2cli_test_walk_sys_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("$RECYCLE.BIN")).unwrap();
        std::fs::write(tmp.join("Thumbs.db"), b"x").unwrap();
        std::fs::write(tmp.join("desktop.ini"), b"x").unwrap();
        std::fs::write(tmp.join("photo.jpg"), b"x").unwrap();

        let mut entries: Vec<(PathBuf, bool)> = Vec::new();
        let mut skipped = 0u32;
        walk_dir(&tmp, &mut entries, &mut skipped).unwrap();
        let files: Vec<_> = entries
            .iter()
            .filter(|(_, is_dir)| !is_dir)
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, vec!["photo.jpg".to_string()]);
        assert_eq!(skipped, 3);

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_upload_pieces() {
        let pieces = |name: &str, size, part| upload_pieces(name, size, part).unwrap();
        assert_eq!(pieces("a.iso", 10, 10), vec![("a.iso".into(), 0, 10)]);
        assert_eq!(pieces("empty", 0, 10), vec![("empty".into(), 0, 0)]);
        assert_eq!(
            pieces("a.iso", 25, 10),
            vec![
                ("a.iso.001".into(), 0, 10),
                ("a.iso.002".into(), 10, 10),
                ("a.iso.003".into(), 20, 5),
            ]
        );
        // An exact multiple gets smaller parts, so the last one is
        // still smaller than the others.
        assert_eq!(
            pieces("a.iso", 20, 10),
            vec![
                ("a.iso.001".into(), 0, 9),
                ("a.iso.002".into(), 9, 9),
                ("a.iso.003".into(), 18, 2),
            ]
        );
        for size in 11..400u64 {
            let p = pieces("f", size, 10);
            let (last, rest) = p.split_last().unwrap();
            assert!(rest.iter().all(|x| x.2 == p[0].2 && last.2 < x.2), "{size}");
            assert_eq!(p.iter().map(|x| x.2).sum::<u64>(), size);
        }
        // Too many parts is refused before building them.
        assert!(upload_pieces("huge", u64::MAX, 2).is_err());
    }

    #[test]
    fn test_part_number() {
        assert_eq!(part_number("big.iso", "big.iso.001"), Some(1));
        assert_eq!(part_number("big.iso", "big.iso.120"), Some(120));
        assert_eq!(part_number("big.iso", "big.iso.000"), None);
        assert_eq!(part_number("big.iso", "big.iso.0001"), None);
        assert_eq!(part_number("big.iso", "big.iso.txt"), None);
        assert_eq!(part_number("big.iso", "big.isox001"), None);
        assert_eq!(part_number("big.iso", "other.iso.001"), None);
    }

    #[test]
    fn test_check_existing_parts() {
        let pieces = upload_pieces("f", 25, 10).unwrap(); // 10, 10, 5
        let mut existing = ExistingFiles::new();
        // Nothing there: fine either way.
        assert!(check_existing_parts("f", &pieces, 1, &existing, false).is_ok());
        existing.insert((1, "f.001".into()), vec![(10, 100)]);
        existing.insert((2, "f.002".into()), vec![(10, 200)]); // other folder
                                                               // A part there without --skip-existing: refused.
        assert!(check_existing_parts("f", &pieces, 1, &existing, false).is_err());
        // Resuming the same file: fine.
        assert!(check_existing_parts("f", &pieces, 1, &existing, true).is_ok());
        // A part this file wouldn't upload, or of another size: refused.
        existing.insert((1, "f.004".into()), vec![(1, 400)]);
        assert!(check_existing_parts("f", &pieces, 1, &existing, true).is_err());
        existing.remove(&(1, "f.004".into()));
        existing.insert((1, "f.003".into()), vec![(7, 300)]);
        assert!(check_existing_parts("f", &pieces, 1, &existing, true).is_err());
    }

    #[test]
    fn test_file_parts() {
        let parse =
            |json: &str| -> Vec<crate::api::MediaItem> { serde_json::from_str(json).unwrap() };
        let ids = |media: &[crate::api::MediaItem]| -> Result<Vec<u64>, O2Error> {
            let first = media
                .iter()
                .find(|m| m.name.as_deref() == Some("big.iso.001"))
                .unwrap();
            Ok(file_parts(first, "big.iso", media)?
                .iter()
                .map(|m| m.id)
                .collect())
        };

        // In order, from this folder only, other names ignored.
        let media = parse(
            r#"[
                {"id": 1, "name": "big.iso.001", "folderid": 2, "size": 10},
                {"id": 3, "name": "big.iso.003", "folderid": 2, "size": 5},
                {"id": 2, "name": "big.iso.002", "folderid": 2, "size": 10},
                {"id": 9, "name": "big.iso.004", "folderid": 5, "size": 10},
                {"id": 7, "name": "big.iso.0001", "folderid": 2, "size": 10},
                {"id": 8, "name": "big.iso.txt", "folderid": 2, "size": 10}
            ]"#,
        );
        assert_eq!(ids(&media).unwrap(), vec![1, 2, 3]);

        let err = |json: &str| ids(&parse(json)).unwrap_err().to_string();
        // A gap.
        let e = err(r#"[
                {"id": 1, "name": "big.iso.001", "folderid": 2, "size": 10},
                {"id": 3, "name": "big.iso.003", "folderid": 2, "size": 5}
            ]"#);
        assert!(e.contains("big.iso.002 is missing"), "{e}");
        // The last parts missing: the last one found is full size.
        let e = err(r#"[
                {"id": 1, "name": "big.iso.001", "folderid": 2, "size": 10},
                {"id": 2, "name": "big.iso.002", "folderid": 2, "size": 10}
            ]"#);
        assert!(e.contains("after big.iso.002 are missing"), "{e}");
        // A part from another upload.
        let e = err(r#"[
                {"id": 1, "name": "big.iso.001", "folderid": 2, "size": 10},
                {"id": 2, "name": "big.iso.002", "folderid": 2, "size": 8},
                {"id": 3, "name": "big.iso.003", "folderid": 2, "size": 5}
            ]"#);
        assert!(e.contains("big.iso.002 isn't the same size"), "{e}");
        // Sizes unknown: can't tell it is complete.
        let e = err(r#"[
                {"id": 1, "name": "big.iso.001", "folderid": 2},
                {"id": 2, "name": "big.iso.002", "folderid": 2}
            ]"#);
        assert!(e.contains("are missing"), "{e}");

        // Two parts with the same number: refuse to guess.
        let media = parse(
            r#"[
                {"id": 1, "name": "big.iso.001", "folderid": 2, "size": 10},
                {"id": 2, "name": "big.iso.002", "folderid": 2, "size": 5},
                {"id": 4, "name": "big.iso.002", "folderid": 2, "size": 5}
            ]"#,
        );
        assert!(matches!(ids(&media), Err(O2Error::Ambiguous(_))));
    }

    fn list_opts() -> ListOptions {
        ListOptions {
            all: false,
            tree: false,
            sort: SortKey::Name,
            reverse: false,
            long: false,
            pattern: None,
            limit: None,
            json: false,
        }
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.heic", "IMG_0616.HEIC"));
        assert!(glob_match("IMG_06??.*", "img_0616.heic"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(glob_match("exact.txt", "Exact.TXT"));
        assert!(!glob_match("*.heic", "IMG_0616.jpg"));
        assert!(!glob_match("IMG_06?.HEIC", "IMG_0616.HEIC"));
        assert!(!glob_match("a*b", "a"));
    }

    #[test]
    fn test_select_files_sort_filter_limit() {
        let media: Vec<crate::api::MediaItem> = serde_json::from_str(
            r#"[
                {"id": 1, "name": "b.jpg", "size": 30, "date": 5000, "creationdate": 1000},
                {"id": 2, "name": "A.jpg", "size": 10, "date": 2000},
                {"id": 3, "name": "c.mov", "size": 20, "date": 3000}
            ]"#,
        )
        .unwrap();
        let ids = |files: &[&crate::api::MediaItem]| files.iter().map(|m| m.id).collect::<Vec<_>>();

        // Name sort is case-insensitive
        let (files, total, size) = select_files(media.iter(), &list_opts());
        assert_eq!(ids(&files), [2, 1, 3]);
        assert_eq!((total, size), (3, 60));

        let opts = ListOptions {
            sort: SortKey::Size,
            reverse: true,
            ..list_opts()
        };
        assert_eq!(ids(&select_files(media.iter(), &opts).0), [1, 3, 2]);

        // Date prefers creationdate over the upload date
        let opts = ListOptions {
            sort: SortKey::Date,
            ..list_opts()
        };
        assert_eq!(ids(&select_files(media.iter(), &opts).0), [1, 2, 3]);

        // The totals count every match, not just the ones within the limit
        let opts = ListOptions {
            pattern: Some("*.jpg".into()),
            limit: Some(1),
            ..list_opts()
        };
        let (files, total, size) = select_files(media.iter(), &opts);
        assert_eq!(ids(&files), [2]);
        assert_eq!((total, size), (2, 40));
    }

    #[test]
    fn test_folder_totals_and_subtree() {
        let (mut folders, _, _) = sample_tree();
        folders.push(crate::api::FolderEntry {
            id: 4,
            name: "deep".into(),
            parentid: 2,
        });
        let folder_name: HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();
        let media: Vec<crate::api::MediaItem> = serde_json::from_str(
            r#"[
                {"id": 100, "name": "a.txt", "folderid": 2, "size": 5},
                {"id": 101, "name": "b.txt", "folderid": 4, "size": 7},
                {"id": 102, "name": "c.txt", "folderid": 3, "size": 1},
                {"id": 103, "name": "lost.txt", "folderid": 999, "size": 2}
            ]"#,
        )
        .unwrap();

        assert_eq!(folder_totals(2, &folders, &media, &folder_name), (2, 12));
        assert_eq!(folder_totals(4, &folders, &media, &folder_name), (1, 7));
        // The root counts everything, including files whose folder is unknown
        assert_eq!(folder_totals(1, &folders, &media, &folder_name), (4, 15));
    }

    #[test]
    fn test_format_dates() {
        assert_eq!(format_date(Some(1705314645000)), "2024-01-15 10:30");
        assert_eq!(format_date_iso(Some(1705314645000)), "2024-01-15T10:30:45Z");
        assert_eq!(format_date_iso(None), serde_json::Value::Null);
    }

    #[test]
    fn test_validate_name() {
        assert_eq!(validate_name("  notes.pdf ").unwrap(), "notes.pdf");
        assert_eq!(validate_name("Fotos 2024").unwrap(), "Fotos 2024");
        for bad in ["", "   ", ".", "..", "a/b", "/x"] {
            assert!(validate_name(bad).is_err(), "{:?} should be rejected", bad);
        }
    }

    #[test]
    fn test_parse_storage_space() {
        let body: crate::api::StorageSpace = serde_json::from_str(
            r#"{"quota":10995116277760,"softdeleted":5011,"free":10982145637694,
                "used":12970640066,"nolimit":false,"individual":{"used":1,"softdeleted":2}}"#,
        )
        .unwrap();
        assert_eq!(body.quota, 10995116277760);
        assert_eq!(body.used, 12970640066);
        assert_eq!(body.softdeleted, 5011);
        assert!(!body.nolimit);
    }
}
