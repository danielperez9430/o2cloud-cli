use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::api::{O2Client, UploadMeta};
use crate::config::AuthConfig;
use crate::error::O2Error;
use crate::util::{format_size, format_unix_utc, mime_type_from_name, system_time_to_compact_iso};

mod download_dir;
pub use download_dir::{download_dir, DownloadDirOptions};

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
    /// Only the files directly in the folder, without listing the whole
    /// account (so no subfolder totals).
    pub folder_only: bool,
    /// Only the files with no (known) folder, shown in the root.
    pub no_folder: bool,
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
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();
    if opts.folder_only {
        let target = resolve_target(path, &folders, &folder_name)?;
        let media = client.get_folder_media(target, auth).await?;
        print_folder_contents(&Tree::new(&folders, &folder_name, &media), target, opts);
        return Ok(());
    }
    let media = client.get_all_media(auth).await?;

    if media.is_empty() && folders.is_empty() {
        if opts.json {
            println!("[]");
        } else {
            println!("(empty)");
        }
        return Ok(());
    }

    // Resolve target folder
    let target = resolve_target(path, &folders, &folder_name)?;

    let tree = Tree::new(&folders, &folder_name, &media);
    if opts.tree {
        print_tree(&tree, target, "", true, opts);
    } else if opts.all {
        print_flat_all(&tree, target, opts);
    } else {
        print_folder_contents(&tree, target, opts);
    }

    Ok(())
}

// ------------------------------------------------------------------
// Display helpers
// ------------------------------------------------------------------

/// The folder tree and the files shown in each folder, indexed once so
/// `ls` takes time in proportion to the files and folders rather than to
/// their product (minutes on an account with tens of thousands of files).
///
/// Files without a folder, or whose folder isn't in the folder list (the
/// server can return a fresh upload like that while it's still processing
/// it), are shown in the root, as `find` already shows them — otherwise
/// they'd appear nowhere.
struct Tree<'a> {
    folders: &'a [crate::api::FolderEntry],
    folder_name: &'a HashMap<u64, String>,
    /// Each folder's subfolders, in the order of `folders`.
    children: HashMap<u64, Vec<&'a crate::api::FolderEntry>>,
    /// The folder each file is shown in, in the order of `media`.
    shown_in: Vec<(u64, &'a crate::api::MediaItem)>,
    /// The files shown in each folder, in the order of `media`.
    files: HashMap<u64, Vec<&'a crate::api::MediaItem>>,
    /// Paths of the folders asked for so far (see `path`).
    paths: std::cell::RefCell<HashMap<u64, String>>,
}

impl<'a> Tree<'a> {
    fn new(
        folders: &'a [crate::api::FolderEntry],
        folder_name: &'a HashMap<u64, String>,
        media: &'a [crate::api::MediaItem],
    ) -> Self {
        let mut children: HashMap<u64, Vec<&crate::api::FolderEntry>> = HashMap::new();
        for f in folders {
            children.entry(f.parentid).or_default().push(f);
        }
        let root = folders.iter().find(|f| f.name == "/").map(|f| f.id);
        let shown_in: Vec<_> = media
            .iter()
            .filter_map(|m| match m.folderid {
                Some(fid) if folder_name.contains_key(&fid) => Some((fid, m)),
                _ => root.map(|root| (root, m)),
            })
            .collect();
        let mut files: HashMap<u64, Vec<&crate::api::MediaItem>> = HashMap::new();
        for &(folder, m) in &shown_in {
            files.entry(folder).or_default().push(m);
        }
        Tree {
            folders,
            folder_name,
            children,
            shown_in,
            files,
            paths: Default::default(),
        }
    }

    /// The files shown in `folder` itself.
    fn files_in(&self, folder: u64) -> impl Iterator<Item = &'a crate::api::MediaItem> + '_ {
        self.files.get(&folder).into_iter().flatten().copied()
    }

    /// The subfolders of `folder`.
    fn children(&self, folder: u64) -> impl Iterator<Item = &'a crate::api::FolderEntry> + '_ {
        self.children.get(&folder).into_iter().flatten().copied()
    }

    /// `folder` and every folder below it.
    fn subtree(&self, folder: u64) -> std::collections::HashSet<u64> {
        let mut ids = std::collections::HashSet::from([folder]);
        let mut stack = vec![folder];
        while let Some(id) = stack.pop() {
            for child in self.children(id) {
                if ids.insert(child.id) {
                    stack.push(child.id);
                }
            }
        }
        ids
    }

    /// The files in `folder` and below, in the order of `media`.
    fn files_under(&self, folder: u64) -> impl Iterator<Item = &'a crate::api::MediaItem> + '_ {
        let ids = self.subtree(folder);
        self.shown_in
            .iter()
            .filter(move |(f, _)| ids.contains(f))
            .map(|&(_, m)| m)
    }

    /// Number of files and their total size in `folder` and below.
    fn totals(&self, folder: u64) -> (usize, u64) {
        self.subtree(folder)
            .iter()
            .flat_map(|&id| self.files_in(id))
            .fold((0, 0), |(n, size), m| (n + 1, size + m.size.unwrap_or(0)))
    }

    /// `resolve_path`, remembered per folder.
    fn path(&self, folder: u64) -> String {
        if let Some(path) = self.paths.borrow().get(&folder) {
            return path.clone();
        }
        let path = resolve_path(folder, self.folder_name, self.folders);
        self.paths.borrow_mut().insert(folder, path.clone());
        path
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
    // By the chosen key, then by name ignoring case, then exactly. The
    // keys are worked out once per file, not once per comparison.
    files.sort_by_cached_key(|m| {
        let primary = match opts.sort {
            SortKey::Name => None,
            SortKey::Size => m.size,
            SortKey::Date => file_date(m),
        };
        let name = m.name.as_deref().unwrap_or("");
        (primary, name.to_lowercase(), name.to_string())
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
/// Set once a command has printed its JSON result, so an error after it
/// isn't printed as a second JSON value (stdout holds one).
pub static PRINTED_JSON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn print_json(v: &serde_json::Value) {
    PRINTED_JSON.store(true, std::sync::atomic::Ordering::Relaxed);
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

fn print_folder_contents(tree: &Tree, target: u64, opts: &ListOptions) {
    let path = tree.path(target);

    let mut child_folders: Vec<_> = tree
        .children(target)
        .filter(|f| f.name != "/" && name_matches(&f.name, opts) && !opts.no_folder)
        .collect();
    sort_folders(&mut child_folders);
    // Without the whole account (`folder_only`), what's in a subfolder
    // isn't known.
    let folder_sizes: Vec<Option<(usize, u64)>> = child_folders
        .iter()
        .map(|f| (!opts.folder_only).then(|| tree.totals(f.id)))
        .collect();
    let (files, total, total_size) = select_files(
        tree.files_in(target).filter(|m| {
            !opts.no_folder
                || !m
                    .folderid
                    .is_some_and(|f| tree.folder_name.contains_key(&f))
        }),
        opts,
    );

    if opts.json {
        let folders_json: Vec<_> = child_folders
            .iter()
            .zip(&folder_sizes)
            .map(|(f, totals)| {
                let (n, size) = totals.unzip();
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
    for (f, totals) in child_folders.iter().zip(&folder_sizes) {
        match totals {
            Some((n, size)) => println!(
                "  {:<id_width$}  📁 {:<name_width$}  {}, {}",
                f.id,
                format!("{}/", f.name),
                plural(*n, "file", "files"),
                format_size(*size),
            ),
            None => println!("  {:<id_width$}  📁 {}/", f.id, f.name),
        }
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

    // The total size includes what's inside the subfolders, when known.
    let folders_size: u64 = folder_sizes.iter().flatten().map(|&(_, size)| size).sum();
    print_summary(
        child_folders.len(),
        files.len(),
        total,
        total_size + folders_size,
    );
    if opts.folder_only && !child_folders.is_empty() {
        println!("  (not counting what's in the subfolders: --folder-only)");
    }
}

fn print_flat_all(tree: &Tree, target: u64, opts: &ListOptions) {
    let (files, total, total_size) = select_files(tree.files_under(target), opts);
    let full_path = |item: &crate::api::MediaItem| {
        let name = item.name.as_deref().unwrap_or("?");
        let path = item
            .folderid
            .filter(|fid| tree.folder_name.contains_key(fid))
            .map(|fid| tree.path(fid))
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

fn print_tree(tree: &Tree, current: u64, prefix: &str, is_last: bool, opts: &ListOptions) {
    let name = tree
        .folder_name
        .get(&current)
        .map(|s| s.as_str())
        .unwrap_or("?");

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
    let (here, _, _) = select_files(tree.files_in(current), opts);

    // Child folders (excluding root which has parentid=0)
    let mut children: Vec<_> = tree
        .children(current)
        .filter(|f| f.name != "/" && f.parentid != 0)
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
        print_tree(tree, child.id, &new_prefix, last, opts);
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
#[derive(Clone)]
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
    /// `upload-dir --update`: also upload the files that changed locally
    /// (see `decide`), replacing the cloud's copy.
    pub update: bool,
    /// `upload-dir --dry-run`: only say what would be done.
    pub dry_run: bool,
}

/// How much newer a local file must be than the cloud's copy to count as
/// changed: filesystems like FAT keep times to 2 seconds.
const MODIFY_WINDOW_MS: u64 = 2000;

/// How long `upload-dir` waits, before deciding what to upload, for files
/// uploaded moments ago to show up in the listing (they take 10–60 s).
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// What `upload-dir` does with one local file, given what the cloud has.
#[derive(Debug, Clone, PartialEq)]
enum Decision {
    /// Not in the cloud (or its folder doesn't exist yet): upload it.
    New,
    /// In the cloud with the same size, and not newer locally.
    Unchanged,
    /// Older locally than the cloud's copy (`cloud`: its IDs): left alone.
    Older { cloud: Vec<u64> },
    /// Changed locally (another size, or newer): with `--update`, upload
    /// it and move these cloud files (the old version) to the trash.
    Changed { old: Vec<u64> },
    /// Another version in the cloud under the names the upload would
    /// take, without `--update`: left alone rather than uploaded as
    /// `name (1)`. `cloud`: the cloud file's ID, or its parts' IDs in
    /// order (`parts`).
    Differs { cloud: Vec<u64>, parts: bool },
    /// Some of its parts are in the cloud: upload the rest.
    Resume,
}

impl Decision {
    /// The word for `--json` (`--dry-run` uses the same ones).
    fn label(&self) -> &'static str {
        match self {
            Decision::New => "upload",
            Decision::Unchanged => "unchanged",
            Decision::Older { .. } => "older",
            Decision::Changed { .. } => "update",
            Decision::Differs { .. } => "differs",
            Decision::Resume => "resume",
        }
    }
}

/// A file's modification time as O2 Cloud stores it (whole seconds, in
/// Unix ms; before 1970 is 0, after 9999 the end of 9999).
fn mtime_ms(t: std::time::SystemTime) -> u64 {
    const END_OF_9999: u64 = 253_402_300_799;
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        .min(END_OF_9999)
        * 1000
}

/// Decide what to do with the local file `name` (`size` bytes, modified
/// at `local_ms`) for the cloud folder `folder` (`None` if it doesn't
/// exist yet). `mdates` has the cloud files' `modificationdate` by ID.
///
/// What the cloud has under the name counts whether it is one file or in
/// parts (`name.001`…), so a file that crossed the part size is replaced
/// too. A file of another size counts as changed only if it isn't older
/// locally: `--update` never puts an older version over a newer one.
/// Without `--skip-existing` everything is uploaded, as `upload-dir` does
/// then; without `--update`, a file of the same size is `Unchanged`
/// whatever its date, and another version (whole or in parts, also when
/// the file crossed the part size) is `Differs`: left alone, since
/// uploading it would leave two versions in the cloud, or a `name (1)`
/// copy. `local_names` are the files next to this one locally: a
/// `name.NNN` among them is never taken for a part of `name`.
#[allow(clippy::too_many_arguments)]
fn decide(
    name: &str,
    size: u64,
    local_ms: u64,
    folder: Option<u64>,
    existing: &ExistingFiles,
    mdates: &HashMap<u64, u64>,
    local_names: &HashSet<String>,
    opts: &UploadOptions,
) -> Decision {
    let Some(folder) = folder else {
        return Decision::New;
    };
    if !opts.skip_existing {
        return Decision::New;
    }
    let Ok(pieces) = upload_pieces(name, size, opts.part_size) else {
        return Decision::New;
    };
    // The whole file and its parts in the cloud, by part number.
    let whole: Vec<(u64, u64)> = existing
        .get(&(folder, name.to_string()))
        .cloned()
        .unwrap_or_default();
    // A `name.NNN` that is also a local file next to this one is that
    // file, not a part of this one.
    let parts: Vec<(usize, &Vec<(u64, u64)>)> = existing
        .parts_of(folder, name)
        .into_iter()
        .filter(|(n, _)| !local_names.contains(&part_name(name, *n)))
        .collect();
    if whole.is_empty() && parts.is_empty() {
        return Decision::New;
    }
    let mut all: Vec<u64> = whole.iter().map(|(_, id)| *id).collect();
    all.extend(parts.iter().flat_map(|(_, f)| f.iter().map(|(_, id)| *id)));
    // The cloud copy that stands for the file: the whole one, or part 1.
    let main_id = if pieces.len() == 1 {
        whole.iter().map(|(_, id)| *id).min()
    } else {
        parts
            .first()
            .and_then(|(_, f)| f.first())
            .map(|(_, id)| *id)
    }
    .or_else(|| whole.iter().map(|(_, id)| *id).min())
    .or_else(|| {
        parts
            .first()
            .and_then(|(_, f)| f.first())
            .map(|(_, id)| *id)
    });
    let cloud_ms = main_id.and_then(|id| mdates.get(&id).copied());
    let older = cloud_ms.is_some_and(|c| local_ms + MODIFY_WINDOW_MS < c);
    let newer = cloud_ms.is_some_and(|c| local_ms > c + MODIFY_WINDOW_MS);
    let older_ids = || main_id.into_iter().collect::<Vec<_>>();
    // Another version: replaced with --update, unless older locally.
    let other_version = || {
        if !opts.update {
            // The cloud's copy: the whole file, or its parts in order.
            return if !whole.is_empty() && (pieces.len() == 1 || parts.is_empty()) {
                Decision::Differs {
                    cloud: whole.iter().map(|(_, id)| *id).min().into_iter().collect(),
                    parts: false,
                }
            } else {
                Decision::Differs {
                    cloud: parts
                        .iter()
                        .filter_map(|(_, f)| f.iter().map(|(_, id)| *id).min())
                        .collect(),
                    parts: true,
                }
            };
        }
        if older {
            Decision::Older { cloud: older_ids() }
        } else {
            Decision::Changed { old: all.clone() }
        }
    };
    // The same size: by date.
    let same_size = || {
        if newer && opts.update {
            Decision::Changed { old: all.clone() }
        } else if older && opts.update {
            Decision::Older { cloud: older_ids() }
        } else {
            Decision::Unchanged
        }
    };

    if pieces.len() == 1 {
        let same = whole.iter().any(|(s, _)| *s == size);
        return if same && parts.is_empty() {
            same_size()
        } else if same {
            // Plus stray parts: only --update tidies them away.
            match same_size() {
                Decision::Unchanged if opts.update => Decision::Changed { old: all.clone() },
                d => d,
            }
        } else {
            other_version()
        };
    }
    let complete = whole.is_empty()
        && parts.len() == pieces.len()
        && parts.iter().all(|(n, f)| {
            matches!((pieces.get(n - 1), f.as_slice()), (Some((_, _, len)), [(s, _)]) if s == len)
        });
    if complete {
        same_size()
    } else if whole.is_empty()
        && check_existing_parts(name, &pieces, folder, existing, true).is_ok()
    {
        Decision::Resume
    } else {
        other_version()
    }
}

/// Files already in the cloud, by (folder ID, name): their sizes and IDs;
/// with an index of the parts (`name.001`…) by the name they're parts
/// of, so finding a file's parts doesn't go through every file.
#[derive(Default)]
struct ExistingFiles {
    files: HashMap<(u64, String), Vec<(u64, u64)>>,
    parts: HashMap<(u64, String), std::collections::BTreeSet<usize>>,
}

/// What the server calls a second upload of `name`: `a (1).txt`.
fn duplicate_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{} (1).{}", stem, ext),
        _ => format!("{} (1)", name),
    }
}

/// `name.001` → (`name`, 1).
fn split_part_name(name: &str) -> Option<(&str, usize)> {
    let base = name.get(..name.len().checked_sub(4)?)?;
    part_number(base, name)
        .filter(|_| !base.is_empty())
        .map(|n| (base, n))
}

impl ExistingFiles {
    fn new() -> Self {
        Self::default()
    }

    fn get(&self, key: &(u64, String)) -> Option<&Vec<(u64, u64)>> {
        self.files.get(key)
    }

    fn add(&mut self, folder: u64, name: String, size: u64, id: u64) {
        if let Some((base, n)) = split_part_name(&name) {
            self.parts
                .entry((folder, base.to_string()))
                .or_default()
                .insert(n);
        }
        self.files
            .entry((folder, name))
            .or_default()
            .push((size, id));
    }

    #[cfg(test)]
    fn insert(&mut self, key: (u64, String), found: Vec<(u64, u64)>) {
        self.remove(&key);
        for (size, id) in found {
            self.add(key.0, key.1.clone(), size, id);
        }
    }

    #[cfg(test)]
    fn remove(&mut self, key: &(u64, String)) {
        if self.files.remove(key).is_some() {
            if let Some((base, n)) = split_part_name(&key.1) {
                if let Some(set) = self.parts.get_mut(&(key.0, base.to_string())) {
                    set.remove(&n);
                }
            }
        }
    }

    /// The parts of `name` in `folder`, by part number.
    fn parts_of(&self, folder: u64, name: &str) -> Vec<(usize, &Vec<(u64, u64)>)> {
        let Some(numbers) = self.parts.get(&(folder, name.to_string())) else {
            return Vec::new();
        };
        numbers
            .iter()
            .filter_map(|&n| {
                self.files
                    .get(&(folder, part_name(name, n)))
                    .map(|f| (n, f))
            })
            .collect()
    }
}

fn existing_files(media: &[crate::api::MediaItem]) -> ExistingFiles {
    let mut existing = ExistingFiles::new();
    for m in media {
        if let (Some(folder), Some(name)) = (m.folderid, m.name.clone()) {
            existing.add(folder, name, m.size.unwrap_or(0), m.id);
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
    // The parts are named `name.001`…, 4 bytes longer than the file.
    if let Some(reason) = crate::api::file_name_problem(&part_name(name, 1)) {
        return Err(O2Error::Invalid(format!(
            "{} goes up in parts, and their names are too long: {}",
            name, reason
        )));
    }
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
    let there = existing.parts_of(folder_id, name);
    if there.is_empty() {
        return Ok(());
    }
    if !skip_existing {
        return Err(O2Error::Conflict(format!(
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
            return Err(O2Error::Conflict(format!(
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
    /// Not uploaded: an empty image, which O2 Cloud rejects (see
    /// `is_empty_image`).
    empty: bool,
}

/// Image types O2 Cloud rejects when the file is empty (`MED-1050`, or
/// `MED-1000` for HEIC). Tested: it takes other empty files, including
/// `.heif`, `.avif`, `.svg`, `.bmp`, `.tif`, `.ico`, `.psd` and RAW.
const REJECTED_IF_EMPTY: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "heic"];

/// Whether a file `name` of `size` bytes is an empty image O2 Cloud
/// would reject, so it isn't sent.
fn is_empty_image(name: &str, size: u64) -> bool {
    size == 0
        && Path::new(name).extension().is_some_and(|e| {
            REJECTED_IF_EMPTY.contains(&e.to_string_lossy().to_lowercase().as_str())
        })
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
    done: &mut Vec<u64>,
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
        empty: false,
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
            Ok(id) => {
                uploaded.ids.push(id);
                done.push(id);
            }
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
            Err(e) => return Err(explain_rejection(e, path)),
        }
    }
    Ok(uploaded)
}

/// What became of one file in `upload-dir`.
enum FileOutcome {
    Uploaded(Uploaded),
    /// Uploaded to replace the cloud files listed (now in the trash).
    Updated(Uploaded, Vec<u64>),
    /// Older locally than in the cloud (these files): left alone.
    Older(Vec<u64>),
    /// Another size than the cloud's copy (its ID, or its parts' IDs if
    /// `true`), without `--update`: left alone.
    Differs(Vec<u64>, bool),
    /// `--dry-run`: what would be done.
    Planned(Decision),
}

/// Upload a changed file and replace the cloud's copy `old` with it. The
/// server renames an upload whose name is taken (`a (1).jpg`, and
/// `a.jpg (1).001` for parts), so: upload, and only once that worked move
/// `old` to the trash and give the new files their names. A failed upload
/// leaves the old version untouched.
#[allow(clippy::too_many_arguments)]
async fn replace_file(
    client: &O2Client,
    auth: &AuthConfig,
    path: &Path,
    name: &str,
    size: u64,
    modified: Option<std::time::SystemTime>,
    folder_id: u64,
    opts: &UploadOptions,
    old: &[u64],
) -> Result<Uploaded, O2Error> {
    let fresh = UploadOptions {
        skip_existing: false,
        ..opts.clone()
    };
    // What has gone up so far, to take it back if the upload fails
    // halfway (parts of a split file): the old version stays as it was.
    let mut done = Vec::new();
    let up = match upload_local_file(
        client,
        auth,
        path,
        name,
        size,
        modified,
        folder_id,
        &ExistingFiles::new(),
        &fresh,
        &mut done,
    )
    .await
    {
        Ok(up) => up,
        Err(e) => {
            if !done.is_empty() && !matches!(e, O2Error::SessionExpired) {
                let _ = client.delete_files(&done, auth).await;
            }
            return Err(e);
        }
    };
    let ids: Vec<String> = up.ids.iter().map(u64::to_string).collect();
    match client.delete_files(old, auth).await {
        Ok(()) => {}
        Err(O2Error::SessionExpired) => return Err(O2Error::SessionExpired),
        // Take the new version back, so nothing is left half done.
        Err(e) => {
            let undone = client.delete_files(&up.ids, auth).await.is_ok();
            return Err(O2Error::Api(format!(
                "the old version of {} couldn't be moved to the trash ({}); {}",
                name,
                e,
                if undone {
                    "the new one was taken back, so nothing changed".to_string()
                } else {
                    format!("the new one is up too (ids: {})", ids.join(", "))
                }
            )));
        }
    }
    for (k, id) in up.ids.iter().enumerate() {
        let new_name = if up.parts > 0 {
            part_name(name, k + 1)
        } else {
            name.to_string()
        };
        let changes = serde_json::json!({ "name": new_name });
        let mut result = client
            .update_file(*id, &new_name, changes.clone(), auth)
            .await;
        if matches!(result, Err(ref e) if !matches!(e, O2Error::SessionExpired)) {
            result = client.update_file(*id, &new_name, changes, auth).await;
        }
        match result {
            Ok(()) => {}
            Err(O2Error::SessionExpired) => return Err(O2Error::SessionExpired),
            Err(e) => {
                return Err(O2Error::Api(format!(
                    "the new version of {} is up (ids: {}) and the old one is in the trash, \
                     but it couldn't be renamed to {}: {}",
                    name,
                    ids.join(", "),
                    new_name,
                    e
                )))
            }
        }
    }
    Ok(up)
}

/// The folder `name` inside `parent`, if it exists.
fn find_folder(folders: &[crate::api::FolderEntry], name: &str, parent: u64) -> Option<u64> {
    folders
        .iter()
        .find(|f| f.name == name && f.parentid == parent)
        .map(|f| f.id)
}

/// `err` with the reason, if O2 Cloud rejected the JPEG at `path`
/// (`MED-1000`) because of a colour profile it can't read.
fn explain_rejection(err: O2Error, path: &Path) -> O2Error {
    let is_jpeg = path.extension().is_some_and(|e| {
        let e = e.to_string_lossy().to_lowercase();
        e == "jpg" || e == "jpeg"
    });
    match err {
        O2Error::Api(msg) if is_jpeg && msg.contains("MED-1000") => {
            match crate::jpeg::read_header(path)
                .ok()
                .and_then(|b| crate::jpeg::profile_problem(&b))
            {
                Some(problem) => O2Error::Api(format!(
                    "{} — {}",
                    msg.split(" — ").next().unwrap_or(&msg),
                    crate::jpeg::describe(&problem)
                )),
                None => O2Error::Api(msg),
            }
        }
        other => other,
    }
}

/// Check the JPEGs in `paths` (files, or folders searched recursively)
/// for colour profiles O2 Cloud can't read, and with `apply` repair the
/// ones that can be (see `crate::jpeg`). A repaired file is written next
/// to the original and renamed over it, keeping its modification date.
pub fn fix_jpeg(paths: &[PathBuf], apply: bool, json: bool) -> Result<(), O2Error> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut entries = Vec::new();
            let mut skipped = 0;
            walk_dir(path, &mut entries, &mut skipped)?;
            files.extend(entries.into_iter().filter(|(_, d)| !d).map(|(p, _)| p));
        } else if path.exists() {
            files.push(path.clone());
        } else {
            return Err(O2Error::NotFound(format!("{}", path.display())));
        }
    }
    files.retain(|p| {
        p.extension().is_some_and(|e| {
            let e = e.to_string_lossy().to_lowercase();
            e == "jpg" || e == "jpeg"
        })
    });

    let (mut mislabelled, mut fixed, mut damaged, mut failed) = (0, 0, 0, 0);
    let mut results = Vec::new();
    for path in &files {
        let Ok(bytes) = crate::jpeg::read_header(path) else {
            eprintln!("  ⚠ Can't read {}", path.display());
            failed += 1;
            continue;
        };
        let Some(problem) = crate::jpeg::profile_problem(&bytes) else {
            continue;
        };
        let mut entry = serde_json::json!({ "path": path.display().to_string() });
        match problem {
            crate::jpeg::ProfileProblem::Mislabelled { chunks } => {
                mislabelled += 1;
                entry["problem"] = "mislabelled".into();
                entry["chunks"] = chunks.into();
                if apply {
                    // Only the header was read: the whole file now.
                    let whole = std::fs::read(path).map_err(O2Error::Io);
                    match whole.and_then(|whole| rewrite_keeping_date(path, &whole)) {
                        Ok(()) => {
                            fixed += 1;
                            eprintln!(
                                "  ✓ {}: colour profile relabelled ({} chunks)",
                                path.display(),
                                chunks
                            );
                        }
                        Err(e) => {
                            failed += 1;
                            eprintln!("  ⚠ {}: couldn't repair it: {}", path.display(), e);
                            entry["error"] = e.to_string().into();
                        }
                    }
                } else {
                    eprintln!(
                        "  • {}: colour profile split into {} chunks all labelled \"1 of 1\"",
                        path.display(),
                        chunks
                    );
                }
                entry["fixed"] = (apply && entry.get("error").is_none()).into();
            }
            crate::jpeg::ProfileProblem::Broken { chunks } => {
                damaged += 1;
                eprintln!(
                    "  ✗ {}: colour profile damaged ({} chunks); can't be repaired safely — \
                     save it again from an image editor",
                    path.display(),
                    chunks
                );
                entry["problem"] = "damaged".into();
                entry["chunks"] = chunks.into();
                entry["fixed"] = false.into();
            }
        }
        results.push(entry);
    }

    eprintln!("Checked {} JPEG file(s).", files.len());
    if mislabelled == 0 && damaged == 0 {
        eprintln!("✓ None has a colour profile O2 Cloud can't read.");
    }
    if mislabelled > 0 && apply {
        eprintln!(
            "✓ Repaired {} of {} with a mislabelled colour profile.",
            fixed, mislabelled
        );
    } else if mislabelled > 0 {
        eprintln!(
            "→ {} with a mislabelled colour profile; run again with --apply to repair them \
             (only the labels change, not the image).",
            mislabelled
        );
    }
    if damaged > 0 {
        eprintln!(
            "⚠ {} with a damaged colour profile; save them again from an image editor.",
            damaged
        );
    }
    if json {
        print_json(&serde_json::Value::Array(results));
    }
    if failed > 0 {
        return Err(O2Error::Invalid(format!(
            "{} file(s) couldn't be read or repaired",
            failed
        )));
    }
    Ok(())
}

/// Replace `path` with its repaired contents: written to a temporary
/// file in the same folder, checked, given the original's modification
/// date and permissions, then renamed over it.
fn rewrite_keeping_date(path: &Path, original: &[u8]) -> Result<(), O2Error> {
    let fixed = crate::jpeg::repair(original)
        .ok_or_else(|| O2Error::Invalid("nothing to repair".into()))?;
    let meta = std::fs::metadata(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{}.o2fix-{}", name, std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, &fixed)?;
        file.sync_all()?;
        if let Ok(modified) = meta.modified() {
            file.set_modified(modified)?;
        }
        drop(file);
        std::fs::set_permissions(&tmp, meta.permissions())?;
        if std::fs::read(&tmp)? != fixed {
            return Err(std::io::Error::other("the written file doesn't match"));
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Whether uploading needs the cloud's file listing: to skip what is
/// already there, or to check for old parts before a split upload.
fn needs_listing(opts: &UploadOptions, largest_file: u64) -> bool {
    opts.skip_existing || opts.update || opts.dry_run || largest_file > opts.part_size
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
    if is_empty_image(&name, size) {
        return Err(O2Error::Invalid(format!(
            "{} is empty, and O2 Cloud rejects empty .{} images",
            name,
            REJECTED_IF_EMPTY.join(", .")
        )));
    }

    let folder_id = match folder_id {
        Some(id) => id,
        None => root_folder_id(client, auth).await?,
    };
    let existing = if needs_listing(opts, size) {
        existing_files(&client.get_folder_media(folder_id, auth).await?)
    } else {
        ExistingFiles::new()
    };
    // --skip-existing never leaves a second version next to the first.
    let decision = decide(
        &name,
        size,
        0,
        Some(folder_id),
        &existing,
        &HashMap::new(),
        &HashSet::new(),
        opts,
    );
    if let Decision::Differs { .. } = decision {
        return Err(O2Error::Conflict(format!(
            "this folder already has {} with another size: not uploaded, so as not to \
             leave two versions (the new one as `{}`). Move the cloud's copy to the trash \
             first (`o2cloud rm`), or upload without --skip-existing to keep both",
            name,
            duplicate_name(&name)
        )));
    }

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
        &mut Vec::new(),
    )
    .await?;

    crate::cache::note_uploaded(&auth.msisdn, &uploaded.ids);
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
/// a time. With `extensions` (see `normalize_extensions`), only files
/// with one of them, and only the subfolders that hold some.
pub async fn upload_dir(
    client: &O2Client,
    auth: &AuthConfig,
    local_dir: &Path,
    parent_folder_id: Option<u64>,
    opts: &UploadOptions,
    extensions: &[String],
    json: bool,
) -> Result<(), O2Error> {
    use futures_util::StreamExt;

    // The folder list, asked for once: for the target and to find the
    // folders that already exist.
    let existing_folders = client.get_all_folders(auth).await?;
    let root_parent = match parent_folder_id {
        Some(id) => id,
        None => existing_folders
            .iter()
            .find(|f| f.name == "/")
            .map(|f| f.id)
            .ok_or_else(|| O2Error::NotFound("Root folder not found".into()))?,
    };

    // Collect all files and directories to upload, skipping hidden and
    // system entries (see `is_skipped_name`).
    // The walk is synchronous filesystem work, so keep it off the runtime.
    let dir = local_dir.to_path_buf();
    let wanted = extensions.to_vec();
    let (entries, skipped_entries, total_size, largest_file) = spawn_blocking_io(move || {
        let mut entries: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
        let mut skipped_entries = 0u32;
        walk_dir(&dir, &mut entries, &mut skipped_entries)?;
        if !wanted.is_empty() {
            keep_extensions(&mut entries, &dir, &wanted);
        }
        let sizes: Vec<u64> = entries
            .iter()
            .filter(|(_, is_dir)| !is_dir)
            .filter_map(|(p, _)| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .collect();
        let largest_file = sizes.iter().copied().max().unwrap_or(0);
        let total_size = if wanted.is_empty() {
            dir_size(&dir)
        } else {
            sizes.iter().sum()
        };
        Ok((entries, skipped_entries, total_size, largest_file))
    })
    .await?;
    let (existing, mdates) = if needs_listing(opts, largest_file) {
        // Uploads of the last minute may not be listed yet: wait for
        // them rather than upload them again as `name (1)`.
        let (media, unsettled) = client.get_all_media_settled(auth, SETTLE_WAIT).await?;
        if unsettled > 0 {
            eprintln!(
                "⚠ {} file(s) uploaded moments ago still aren't listed; if any of them \
                 is in this folder it may be uploaded again as `name (1)`",
                unsettled
            );
        }
        let mdates: HashMap<u64, u64> = media
            .iter()
            .filter_map(|m| m.modificationdate.map(|d| (m.id, d)))
            .collect();
        (existing_files(&media), mdates)
    } else {
        (ExistingFiles::new(), HashMap::new())
    };

    // Count files for progress
    let file_count = entries.iter().filter(|(_, is_dir)| !is_dir).count();
    let only = extensions
        .iter()
        .map(|e| format!(".{}", e))
        .collect::<Vec<_>>()
        .join(", ");
    if !extensions.is_empty() && file_count == 0 {
        return Err(O2Error::NotFound(format!(
            "no {} files in {}",
            only,
            local_dir.display()
        )));
    }
    // The uploaded directory becomes a folder of the same name inside the
    // target (like `cp -r`); `canonicalize` gives `.`/`..` their real name.
    let top_name = local_dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    // O2 Cloud's limits on names and depth: what doesn't fit is reported
    // and left out, rather than failing the whole upload halfway.
    let top_level = folder_level(root_parent, &existing_folders) + usize::from(top_name.is_some());
    if let Some(name) = &top_name {
        if let Some(problem) = crate::api::folder_name_problem(name) {
            return Err(O2Error::Invalid(format!(
                "can't upload {}: {}",
                name, problem
            )));
        }
        if top_level > crate::api::MAX_FOLDER_LEVEL {
            return Err(O2Error::Invalid(format!(
                "can't upload {} there: O2 Cloud allows at most {} levels of folders below the root",
                name,
                crate::api::MAX_FOLDER_LEVEL
            )));
        }
    }
    let mut entries = entries;
    let rejected = keep_within_o2_limits(&mut entries, local_dir, top_level);
    for (path, is_dir, reason) in &rejected {
        if *is_dir {
            eprintln!(
                "  ⚠ Skipping {}/ and what's in it: {}",
                local_path_in(local_dir, path),
                reason
            );
        } else if reason.starts_with("the name") {
            eprintln!(
                "  ⚠ Skipping {}: {}",
                local_path_in(local_dir, path),
                reason
            );
        }
    }
    let file_count = entries.iter().filter(|(_, is_dir)| !is_dir).count();
    eprintln!(
        "→ {} {} ({}) — {} files, {} dirs{}...",
        if opts.dry_run {
            "Checking (dry run)"
        } else {
            "Uploading"
        },
        local_dir.display(),
        format_size(total_size),
        file_count,
        entries.iter().filter(|(_, is_dir)| *is_dir).count() - 1, // -1 for root
        if extensions.is_empty() {
            String::new()
        } else {
            format!(" (only {})", only)
        },
    );

    // Map: local dir path → o2 folder id. The uploaded directory itself
    // becomes a folder of the same name inside the target (like `cp -r`);
    // `canonicalize` gives `.`/`..` their real name.
    // With --dry-run nothing is created: a folder that doesn't exist yet
    // is `None`, and everything in it is new.
    let mut folder_map: HashMap<PathBuf, Option<u64>> = HashMap::new();
    let top_id = match top_name {
        Some(name) if opts.dry_run => find_folder(&existing_folders, &name, root_parent),
        Some(name) => {
            Some(ensure_folder(client, auth, &existing_folders, &name, root_parent).await?)
        }
        // Only `/` has no name: upload its contents into the target.
        None => Some(root_parent),
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
        let folder_id = if opts.dry_run {
            parent_o2.and_then(|p| find_folder(&existing_folders, &name, p))
        } else {
            let parent = parent_o2.unwrap_or(root_parent);
            Some(ensure_folder(client, auth, &existing_folders, &name, parent).await?)
        };
        folder_map.insert(dir_path.clone(), folder_id);
    }

    // Upload files, `opts.jobs` at a time.
    let files: Vec<(usize, &PathBuf, Option<u64>)> = entries
        .iter()
        .filter(|(_, is_dir)| !is_dir)
        .enumerate()
        .map(|(i, (file_path, _))| {
            let parent_local = file_path.parent().unwrap();
            let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(top_id);
            (i, file_path, parent_o2)
        })
        .collect();
    let (existing, mdates) = (&existing, &mdates);
    // The local files of each folder, by name (see `decide`).
    let mut local_names: HashMap<&Path, HashSet<String>> = HashMap::new();
    for (path, is_dir) in &entries {
        if let (false, Some(parent), Some(name)) = (*is_dir, path.parent(), path.file_name()) {
            local_names
                .entry(parent)
                .or_default()
                .insert(name.to_string_lossy().into_owned());
        }
    }
    let local_names = &local_names;
    // Set when the session expires: the files not started yet are left
    // alone rather than each failing the same way.
    let expired = std::sync::atomic::AtomicBool::new(false);
    let expired = &expired;
    let mut results: Vec<(usize, serde_json::Value, Result<FileOutcome, O2Error>)> =
        futures_util::stream::iter(files)
            .map(|(i, file_path, parent_o2)| async move {
                let name = file_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if expired.load(std::sync::atomic::Ordering::Relaxed) {
                    let result = serde_json::json!({
                        "local": local_path_in(local_dir, file_path),
                        "name": name,
                        "folderid": parent_o2,
                    });
                    return (i, result, Err(O2Error::SessionExpired));
                }
                let file_meta = tokio::fs::metadata(file_path).await.ok();
                let file_meta_ok = file_meta.is_some();
                let size = file_meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = file_meta.and_then(|m| m.modified().ok());

                if !opts.dry_run {
                    eprintln!(
                        "  [{}/{}] 📄 {} ({})",
                        i + 1,
                        file_count,
                        name,
                        format_size(size)
                    );
                }

                let mut result = serde_json::json!({
                    "local": local_path_in(local_dir, file_path),
                    "name": name,
                    "folderid": parent_o2,
                    "size": size,
                });
                if file_meta_ok && is_empty_image(&name, size) {
                    eprintln!(
                        "  ∅ {} is empty: skipped (O2 Cloud rejects empty images)",
                        name
                    );
                    result["empty"] = true.into();
                    if opts.dry_run {
                        result["dry_run"] = true.into();
                    }
                    let empty = Uploaded {
                        ids: Vec::new(),
                        parts: 0,
                        skipped: 0,
                        empty: true,
                    };
                    return (i, result, Ok(FileOutcome::Uploaded(empty)));
                }
                let decision = (opts.skip_existing || opts.dry_run).then(|| {
                    let local_ms = modified.map(mtime_ms).unwrap_or(0);
                    let siblings = file_path.parent().and_then(|p| local_names.get(p));
                    let none = HashSet::new();
                    let siblings = siblings.unwrap_or(&none);
                    decide(
                        &name, size, local_ms, parent_o2, existing, mdates, siblings, opts,
                    )
                });
                let local = local_path_in(local_dir, file_path);
                if let (true, Some(decision)) = (opts.dry_run, &decision) {
                    match &decision {
                        Decision::New => eprintln!("  + {}", local),
                        Decision::Changed { .. } => eprintln!("  ↻ {} (changed locally)", local),
                        Decision::Resume => eprintln!("  … {} (parts missing)", local),
                        Decision::Differs { .. } => eprintln!(
                            "  ≠ {} (another size than in the cloud; --update replaces it)",
                            local
                        ),
                        _ => {}
                    }
                    result["action"] = decision.label().into();
                    result["dry_run"] = true.into();
                    match decision {
                        Decision::Changed { old } => result["replaces"] = old.clone().into(),
                        // The cloud's copy, as in the real run.
                        Decision::Older { cloud } if cloud.len() > 1 => {
                            result["parts"] = cloud.clone().into()
                        }
                        Decision::Differs { cloud, parts: true } => {
                            result["parts"] = cloud.clone().into()
                        }
                        Decision::Older { cloud } | Decision::Differs { cloud, .. }
                            if !cloud.is_empty() =>
                        {
                            result["id"] = cloud[0].into()
                        }
                        _ => {}
                    }
                    let decision = decision.clone();
                    return (i, result, Ok(FileOutcome::Planned(decision)));
                }
                let folder = parent_o2.unwrap_or(root_parent);
                let uploaded = match decision {
                    Some(Decision::Changed { old }) => {
                        eprintln!("  ↻ {} changed locally: replacing the cloud's copy", local);
                        replace_file(
                            client, auth, file_path, &name, size, modified, folder, opts, &old,
                        )
                        .await
                        .map(|up| FileOutcome::Updated(up, old))
                    }
                    Some(Decision::Older { cloud }) => {
                        eprintln!(
                            "  = {} is older locally than in the cloud: left alone",
                            local
                        );
                        Ok(FileOutcome::Older(cloud))
                    }
                    Some(Decision::Differs { cloud, parts }) => {
                        eprintln!(
                            "  ≠ {} has another size than in the cloud: left alone \
                             (--update replaces it)",
                            local
                        );
                        Ok(FileOutcome::Differs(cloud, parts))
                    }
                    _ => upload_local_file(
                        client,
                        auth,
                        file_path,
                        &name,
                        size,
                        modified,
                        folder,
                        existing,
                        opts,
                        &mut Vec::new(),
                    )
                    .await
                    .map(FileOutcome::Uploaded),
                };
                match &uploaded {
                    Err(O2Error::SessionExpired) => {
                        if !expired.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            eprintln!("  ✗ The session expired while uploading {}", name);
                        }
                    }
                    Err(e) => eprintln!("  ⚠ Upload failed for {}: {}", name, e),
                    Ok(_) => {}
                }
                (i, result, uploaded)
            })
            .buffer_unordered(opts.jobs.max(1))
            .collect()
            .await;
    results.sort_by_key(|(i, _, _)| *i);

    // One entry per file for `--json`, failed uploads included.
    let (mut succeeded, mut skipped, mut failed, mut not_done) = (0usize, 0usize, 0usize, 0usize);
    let (mut empty_images, mut updated, mut older, mut differs) = (0usize, 0usize, 0usize, 0usize);
    // What this run put in the cloud, for the next listing (see below).
    let mut uploaded_ids: Vec<u64> = Vec::new();
    let mut planned: HashMap<&'static str, usize> = HashMap::new();
    let set_ids = |result: &mut serde_json::Value, up: &Uploaded| {
        if up.parts > 0 {
            result["parts"] = up.ids.clone().into();
        } else {
            result["id"] = up.ids[0].into();
        }
    };
    let results: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(_, mut result, uploaded)| {
            match uploaded {
                Ok(FileOutcome::Uploaded(up)) if up.empty => {
                    result["skipped"] = true.into();
                    result["action"] = "empty".into();
                    empty_images += 1;
                }
                Ok(FileOutcome::Uploaded(up)) => {
                    let all_skipped = up.skipped == up.ids.len();
                    if all_skipped {
                        skipped += 1;
                    } else {
                        succeeded += 1;
                    }
                    result["skipped"] = all_skipped.into();
                    result["action"] = if all_skipped { "skipped" } else { "uploaded" }.into();
                    set_ids(&mut result, &up);
                    uploaded_ids.extend(&up.ids);
                }
                Ok(FileOutcome::Updated(up, old)) => {
                    updated += 1;
                    result["skipped"] = false.into();
                    result["action"] = "updated".into();
                    result["replaced"] = old.into();
                    set_ids(&mut result, &up);
                    uploaded_ids.extend(&up.ids);
                }
                Ok(outcome @ (FileOutcome::Older(_) | FileOutcome::Differs(..))) => {
                    let (label, cloud, in_parts) = match outcome {
                        FileOutcome::Older(cloud) => {
                            older += 1;
                            let in_parts = cloud.len() > 1;
                            ("older", cloud, in_parts)
                        }
                        FileOutcome::Differs(cloud, in_parts) => {
                            differs += 1;
                            ("differs", cloud, in_parts)
                        }
                        _ => unreachable!(),
                    };
                    result["skipped"] = true.into();
                    result["action"] = label.into();
                    // The cloud's copy, as `skipped` entries have.
                    if in_parts {
                        result["parts"] = cloud.into();
                    } else if let Some(id) = cloud.first() {
                        result["id"] = (*id).into();
                    }
                }
                Ok(FileOutcome::Planned(d)) => {
                    *planned.entry(d.label()).or_default() += 1;
                }
                Err(O2Error::SessionExpired) => {
                    result["error"] = O2Error::SessionExpired.to_string().into();
                    result["action"] = "failed".into();
                    not_done += 1;
                }
                Err(e) => {
                    result["error"] = e.to_string().into();
                    result["action"] = "failed".into();
                    failed += 1;
                }
            }
            result
        })
        // Kept only for --json: on a large folder they take hundreds of MB.
        .filter(|_| json)
        .collect();
    let beyond_limits: Vec<serde_json::Value> = rejected
        .iter()
        .filter(|(_, is_dir, _)| !is_dir)
        .map(|(path, _, reason)| {
            let mut entry = serde_json::json!({
                "local": local_path_in(local_dir, path),
                "name": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "error": reason,
                "action": "failed",
            });
            if opts.dry_run {
                entry["dry_run"] = true.into();
            }
            entry
        })
        .collect();
    if opts.dry_run {
        let n = |k| planned.get(k).copied().unwrap_or(0);
        eprintln!(
            "Dry run: {} to upload, {} to update, {} to resume, {} unchanged, {} older locally, \
             {}{} empty image(s), {} beyond O2 Cloud's limits; nothing was changed.",
            n("upload"),
            n("update"),
            n("resume"),
            n("unchanged"),
            n("older"),
            if n("differs") > 0 {
                format!(
                    "{} of another size (left alone without --update), ",
                    n("differs")
                )
            } else {
                String::new()
            },
            empty_images,
            beyond_limits.len()
        );
        if json {
            let mut results = results;
            results.extend(beyond_limits);
            print_json(&serde_json::Value::Array(results));
        }
        return Ok(());
    }

    // Listings take up to a minute to include new uploads: the next
    // listing asks for these by ID, so a run right after this one (after
    // the session expired, say) doesn't upload them again.
    crate::cache::note_uploaded(&auth.msisdn, &uploaded_ids);
    let too_big = rejected.iter().filter(|(_, is_dir, _)| !is_dir).count();
    if too_big > 0 {
        eprintln!(
            "⚠ {} file(s) not uploaded: beyond O2 Cloud's limits on names or folder depth",
            too_big
        );
    }
    if skipped_entries > 0 {
        eprintln!(
            "  (skipped {} hidden or system files/dirs: .git, .DS_Store, Thumbs.db, etc.)",
            skipped_entries
        );
    }
    if skipped > 0 {
        eprintln!("  ({} file(s) were already uploaded)", skipped);
    }
    if older > 0 {
        eprintln!(
            "  ({} file(s) are older locally than in the cloud; left alone)",
            older
        );
    }
    if differs > 0 {
        eprintln!(
            "  ({} file(s) have another size than in the cloud; left alone. \
             Use --update to replace the cloud's copies)",
            differs
        );
    }
    if updated > 0 {
        eprintln!(
            "↻ Updated {} changed file(s); their old versions are in the trash",
            updated
        );
    }
    if empty_images > 0 {
        eprintln!(
            "  ({} empty image(s) skipped: O2 Cloud rejects empty .{} files)",
            empty_images,
            REJECTED_IF_EMPTY.join(", .")
        );
    }
    if failed > 0 {
        eprintln!("⚠ {} file(s) failed to upload", failed);
    }
    eprintln!(
        "{} Uploaded {} file(s) from {}{}",
        if failed + too_big + not_done > 0 {
            "⚠"
        } else {
            "✓"
        },
        succeeded,
        local_dir.display(),
        if updated > 0 {
            format!(", and updated {}", updated)
        } else {
            String::new()
        }
    );
    if not_done > 0 {
        eprintln!(
            "✗ {} file(s) not uploaded: the session expired. Run the same command \
             with --skip-existing to log in and carry on.",
            not_done
        );
    }
    if json {
        let mut results = results;
        results.extend(beyond_limits);
        print_json(&serde_json::Value::Array(results));
    }

    if not_done > 0 {
        return Err(O2Error::SessionExpired);
    }
    let failed = failed + too_big;
    if failed > 0 {
        return Err(O2Error::Partial(format!(
            "{} of {} files failed to upload",
            failed,
            file_count + too_big
        )));
    }
    Ok(())
}

/// How many levels below the root folder `id` is (the root is 0).
fn folder_level(id: u64, folders: &[crate::api::FolderEntry]) -> usize {
    let parent: HashMap<u64, u64> = folders.iter().map(|f| (f.id, f.parentid)).collect();
    let (mut level, mut current) = (0, id);
    while let Some(&p) = parent.get(&current) {
        if p == 0 || p == current || level > 1000 {
            break;
        }
        level += 1;
        current = p;
    }
    level
}

/// Leave out of `entries` (as `walk_dir` collects them under `root`) what
/// O2 Cloud won't take, given that `root` becomes a folder at `root_level`:
/// folders deeper than `MAX_FOLDER_LEVEL` or with too long a name, with
/// everything in them, and files with too long a name. Returns each one
/// left out with the reason: the folders once, and every file.
fn keep_within_o2_limits(
    entries: &mut Vec<(PathBuf, bool)>,
    root: &Path,
    root_level: usize,
) -> Vec<(PathBuf, bool, String)> {
    let mut bad_dirs: Vec<(PathBuf, String)> = Vec::new();
    let mut rejected = Vec::new();
    let mut dirs: Vec<&PathBuf> = entries.iter().filter(|(_, d)| *d).map(|(p, _)| p).collect();
    dirs.sort_by_key(|p| p.components().count()); // parents first
    for dir in dirs {
        if dir.as_path() == root || bad_dirs.iter().any(|(b, _)| dir.starts_with(b)) {
            continue;
        }
        let rel = dir.strip_prefix(root).unwrap_or(dir).components().count();
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let reason = crate::api::folder_name_problem(&name).or_else(|| {
            (root_level + rel > crate::api::MAX_FOLDER_LEVEL).then(|| {
                format!(
                    "O2 Cloud allows at most {} levels of folders below the root",
                    crate::api::MAX_FOLDER_LEVEL
                )
            })
        });
        if let Some(reason) = reason {
            rejected.push((dir.clone(), true, reason.clone()));
            bad_dirs.push((dir.clone(), reason));
        }
    }
    entries.retain(|(path, is_dir)| {
        if let Some((dir, reason)) = bad_dirs.iter().find(|(b, _)| path.starts_with(b)) {
            if !is_dir {
                let reason = format!("in {}: {}", local_path_in(root, dir), reason);
                rejected.push((path.clone(), false, reason));
            }
            return false;
        }
        if !is_dir {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(reason) = crate::api::file_name_problem(&name) {
                rejected.push((path.clone(), false, reason));
                return false;
            }
        }
        true
    });
    rejected
}

/// `file_path` inside the uploaded directory `local_dir`, with `/` on
/// every platform.
fn local_path_in(local_dir: &Path, file_path: &Path) -> String {
    file_path
        .strip_prefix(local_dir)
        .unwrap_or(file_path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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

/// The extensions given to `--ext`, lowercase and without the leading
/// `*` or dot (`*.JPG` → `jpg`), leaving out empty ones.
pub fn normalize_extensions(extensions: &[String]) -> Vec<String> {
    extensions
        .iter()
        .map(|e| {
            e.trim()
                .trim_start_matches('*')
                .trim_start_matches('.')
                .to_lowercase()
        })
        .filter(|e| !e.is_empty())
        .collect()
}

/// Keep, of the `entries` `walk_dir` collected under `root`, the files
/// with one of the `wanted` extensions (see `normalize_extensions`), the
/// directories that hold one somewhere below, and `root` itself.
fn keep_extensions(entries: &mut Vec<(PathBuf, bool)>, root: &Path, wanted: &[String]) {
    // By the end of the name, so `tar.gz` works too.
    let is_wanted = |p: &Path| {
        p.file_name().is_some_and(|n| {
            let name = n.to_string_lossy().to_lowercase();
            wanted.iter().any(|e| {
                name.len() > e.len() + 1
                    && name.ends_with(e.as_str())
                    && name[..name.len() - e.len()].ends_with('.')
            })
        })
    };
    entries.retain(|(p, is_dir)| *is_dir || is_wanted(p));
    let mut needed: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for (path, _) in entries.iter().filter(|(_, is_dir)| !is_dir) {
        for dir in path.ancestors().skip(1) {
            if !needed.insert(dir.to_path_buf()) || dir == root {
                break;
            }
        }
    }
    entries.retain(|(p, is_dir)| !is_dir || p == root || needed.contains(p));
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
                // 4 GiB or more needs ZIP64, which the zip crate only
                // writes when told (else it aborts at the 4 GiB mark).
                let large =
                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= u32::MAX as u64;
                zip.start_file(&entry_name, options.large_file(large))
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
    let Lookup { media, .. } =
        media_for_target(client, auth, target, &folders, Needs::File).await?;
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

    // The file is written in place (an app may follow its size as the
    // progress), with a hidden marker next to it saying which download
    // it is until it's complete: a cut download is resumed by running
    // the same command again, and anything else there is never touched.
    let size = pieces.iter().map(|m| m.size).sum::<Option<u64>>();
    let marker = download_marker(&output_path);
    let mark = serde_json::json!({ "ids": ids, "size": size }).to_string();
    let resuming = if output_path.exists() {
        match std::fs::read_to_string(&marker) {
            Ok(found) if found == mark => true,
            _ => {
                return Err(O2Error::Conflict(format!(
                    "Refusing to overwrite existing file: {}",
                    output_path.display()
                )))
            }
        }
    } else {
        false
    };
    crate::config::write_private(&marker, mark.as_bytes()).map_err(|e| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {}", marker.display(), e),
        ))
    })?;
    let mut file = if resuming {
        tokio::fs::File::from_std(download_dir::open_part(&output_path)?)
    } else {
        crate::api::create_new_file(&output_path).await?
    };
    let have = file.metadata().await?.len();
    match (resuming, pieces.len()) {
        (true, _) => eprintln!(
            "→ Resuming {} from {}...",
            output_path.display(),
            format_size(have)
        ),
        (false, 1) => eprintln!("→ Downloading media {}...", ids[0]),
        (false, n) => eprintln!("→ Downloading {} parts ({})...", n, ids[0]),
    }
    // The listing's URL for a single file; parts get a fresh one each,
    // as they may expire during a long join.
    let urls: HashMap<u64, String> = match pieces.as_slice() {
        [item] => item
            .url
            .clone()
            .map(|url| (item.id, url))
            .into_iter()
            .collect(),
        _ => HashMap::new(),
    };
    let bytes_written =
        download_dir::fetch_pieces(client, auth, &pieces, size, &mut file, &output_path, &urls)
            .await
            .inspect_err(|_| {
                eprintln!(
                    "  (the part downloaded stays in {}: run the same command again to resume)",
                    output_path.display()
                );
            })?;
    let mtime = pieces[0].modificationdate.or(pieces[0].creationdate);
    download_dir::finish(file, mtime, &output_path).await?;
    let _ = std::fs::remove_file(&marker);

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

/// The hidden file that marks `path` as a download in progress:
/// `.<name>.o2dl` next to it.
fn download_marker(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{}.o2dl", name))
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
    media: impl IntoIterator<Item = &'a crate::api::MediaItem>,
) -> Result<Vec<&'a crate::api::MediaItem>, O2Error> {
    let mut by_number: HashMap<usize, Vec<&crate::api::MediaItem>> = HashMap::new();
    for m in media.into_iter().filter(|m| m.folderid == first.folderid) {
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

/// How much smaller than the part size a part can be: `upload_pieces`
/// shrinks it a byte at a time while the size is a multiple of it.
const PART_SHRINK_MAX: u64 = 64;

/// The part size given in `O2CLOUD_PART_SIZE`, for tests: with it, files
/// go up in parts of that size, and parts of that size are taken for the
/// CLI's (see `made_by_cli`).
pub fn test_part_size() -> Option<u64> {
    std::env::var("O2CLOUD_PART_SIZE").ok()?.parse().ok()
}

/// Whether a complete set of parts (see `file_parts`) was made by this
/// CLI (or an app using it) rather than being someone's own `x.001`,
/// `x.002`… volumes, uploaded as files of their own: every part but the
/// last is the CLI's part size, which has always been `DEFAULT_PART_SIZE`
/// (at most a few bytes less: see `upload_pieces`).
pub(crate) fn made_by_cli(parts: &[&crate::api::MediaItem]) -> bool {
    let Some(full) = parts.first().and_then(|p| p.size) else {
        return false;
    };
    parts.len() > 1
        && std::iter::once(DEFAULT_PART_SIZE)
            .chain(test_part_size())
            .any(|size| full <= size && full + PART_SHRINK_MAX >= size)
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

    // Files (each folder's path worked out once, not once per file)
    let tree = Tree::new(&folders, &folder_name, &[]);
    for item in &media {
        let name = item.name.as_deref().unwrap_or("");
        if name.to_lowercase().contains(&q) {
            let folder_path = item
                .folderid
                .map(|fid| tree.path(fid))
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
    let Lookup { media, .. } =
        media_for_target(client, auth, target, &folders, Needs::ItemAndFolderContents).await?;
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
        Item::Parts(name, parts) => {
            let ids: Vec<u64> = parts.iter().map(|p| p.id).collect();
            eprintln!("→ Deleting {} ({} parts)...", name, ids.len());
            delete_files_batched(client, auth, &ids).await?;
            eprintln!("✓ Deleted {} ({} parts)", name, ids.len());
            Ok(())
        }
    }
}

/// A file or folder named on the command line.
enum Item<'a> {
    Folder(u64),
    File(&'a crate::api::MediaItem),
    /// A file uploaded in parts, named by its own name (`big.iso` for
    /// `big.iso.001`…): the name and its parts in order.
    Parts(String, Vec<&'a crate::api::MediaItem>),
}

/// Refuse a part of a file the CLI uploaded in parts named on its own
/// (by name or path, not by ID): acting on it alone breaks the file.
fn refuse_lone_part(
    item: &crate::api::MediaItem,
    media: &[crate::api::MediaItem],
) -> Result<(), O2Error> {
    let Some((base, n)) = item.name.as_deref().and_then(split_part_name) else {
        return Ok(());
    };
    let siblings = || media.iter().filter(|m| m.folderid == item.folderid);
    // A file of its own named `base` makes `base.NNN` plain files.
    if siblings().any(|m| m.name.as_deref() == Some(base)) {
        return Ok(());
    }
    let first_name = part_name(base, 1);
    let Some(first) = siblings().find(|m| m.name.as_deref() == Some(first_name.as_str())) else {
        return Ok(());
    };
    match file_parts(first, base, siblings()) {
        Ok(parts) if made_by_cli(&parts) && parts.iter().any(|p| p.id == item.id) => {
            Err(O2Error::Invalid(format!(
                "{} is part {} of {} of {}, which was uploaded in parts: on its own \
                 it would break {}. Name `{}` to act on the whole file, or use this \
                 part's ID ({}) for this part only",
                item.name.as_deref().unwrap_or("?"),
                n,
                parts.len(),
                base,
                base,
                base,
                item.id
            )))
        }
        _ => Ok(()),
    }
}

/// Apply a change to each part in turn: `change` gives the part's name
/// then and the change. If one fails, the ones done are put back as
/// `undo` says (as far as possible), so the file isn't left split.
async fn update_parts(
    client: &O2Client,
    auth: &AuthConfig,
    parts: &[&crate::api::MediaItem],
    change: impl Fn(&crate::api::MediaItem) -> (String, serde_json::Value),
    undo: impl Fn(&crate::api::MediaItem) -> (String, serde_json::Value),
) -> Result<(), O2Error> {
    for (i, part) in parts.iter().enumerate() {
        let (name, changes) = change(part);
        if let Err(e) = client.update_file(part.id, &name, changes, auth).await {
            for done in &parts[..i] {
                let (name, changes) = undo(done);
                let _ = client.update_file(done.id, &name, changes, auth).await;
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Whether `files` has a file named `name`, or a part of one (`name.NNN`),
/// other than the files `own`.
fn name_taken(files: &[crate::api::MediaItem], name: &str, own: &[u64]) -> bool {
    files.iter().any(|m| {
        !own.contains(&m.id)
            && m.name
                .as_deref()
                .is_some_and(|n| n == name || part_number(name, n).is_some())
    })
}

/// What a command can be given, which decides how much of the account
/// `media_for_target` lists.
#[derive(Clone, Copy, PartialEq)]
enum Needs {
    /// Only a file (`download`); a folder ID or path is looked up as a
    /// file name.
    File,
    /// A file, or a folder whose files the command doesn't use.
    Item,
    /// A file, or a folder whose files it does use (`rm` shows and
    /// deletes them).
    ItemAndFolderContents,
}

/// The files `media_for_target` found: the whole account, or with
/// `complete` false only the target (or nothing, for a folder ID).
struct Lookup {
    media: Vec<crate::api::MediaItem>,
    complete: bool,
}

/// How long `media_for_target` waits for a new upload to show up, and
/// how long it waits between asking: from the first delay, each 1.5
/// times the last, up to the longest (about 11 requests in 60 s).
const NEW_UPLOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
const NEW_UPLOAD_FIRST_POLL: std::time::Duration = std::time::Duration::from_secs(1);
const NEW_UPLOAD_LONGEST_POLL: std::time::Duration = std::time::Duration::from_secs(8);

/// How far back the change log is searched for a deletion of an unlisted
/// ID: from a day before the latest upload listed, but no more than 30
/// days back, to keep the answer small.
const DELETED_LOOKBACK_MS: u64 = 24 * 3600 * 1000;
const DELETED_MAX_LOOKBACK_MS: u64 = 30 * 24 * 3600 * 1000;

/// The files in `folder` (`None`: the photos and videos with no folder),
/// to check a name there: taken from `media` if it is `complete` (the
/// whole account), otherwise asked for.
async fn files_in(
    client: &O2Client,
    auth: &AuthConfig,
    folder: Option<u64>,
    media: &[crate::api::MediaItem],
    complete: bool,
) -> Result<Vec<crate::api::MediaItem>, O2Error> {
    let listed;
    let media = match (complete, folder) {
        (true, _) => media,
        (false, Some(id)) => return client.get_folder_media(id, auth).await,
        (false, None) => {
            listed = client.get_all_media(auth).await?;
            &listed
        }
    };
    Ok(media
        .iter()
        .filter(|m| m.folderid == folder)
        .cloned()
        .collect())
}

/// The files needed to resolve `target`. Listing every file takes seconds
/// on a large account, so this lists as little as `resolve_item` (or, for
/// `Needs::File`, `download_file`) needs to resolve `target` the same way:
/// - a folder ID or path: nothing, unless the command uses its files;
/// - a file ID: that one file, asked for by ID;
/// - a file path (`/docs/a.txt`): the files in its folder.
///
/// A bare name, which may be any file in the account, lists everything
/// (`listed_media`), as does anything the shortcuts don't find.
async fn media_for_target(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    folders: &[crate::api::FolderEntry],
    needs: Needs,
) -> Result<Lookup, O2Error> {
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();
    // In `resolve_item`'s order: folder ID, file ID, folder path or name.
    let numeric = target.parse::<u64>().ok();
    let folder_id = numeric.filter(|id| folder_name.contains_key(id));
    let by_id = match numeric {
        // Not found (or an error): left to the listing, which also
        // resolves numeric names and waits for new uploads.
        Some(id) if folder_id.is_none() => client.get_media_by_id(id, auth).await.ok().flatten(),
        _ => None,
    };
    let is_folder = needs != Needs::File
        && by_id.is_none()
        && (folder_id.is_some()
            || resolve_target(Some(target.to_string()), folders, &folder_name).is_ok());
    let shortcut = if let Some(item) = by_id {
        Some(vec![item])
    } else if is_folder {
        (needs == Needs::Item).then(Vec::new)
    } else if let Some((dir, _)) = target.rsplit_once('/') {
        let dir = if dir.is_empty() { "/" } else { dir };
        match resolve_target(Some(dir.to_string()), folders, &folder_name) {
            Ok(folder_id) => Some(client.get_folder_media(folder_id, auth).await?),
            // No such folder: `find_file` says so without a listing.
            Err(_) => Some(Vec::new()),
        }
    } else {
        None
    };
    Ok(match shortcut {
        Some(media) => Lookup {
            media,
            complete: false,
        },
        None => Lookup {
            media: listed_media(client, auth, target, folders).await?,
            complete: true,
        },
    })
}

/// The account's files, for resolving `target`. The listing shows a new
/// upload only after 10–40 s, so a numeric `target` that is neither a
/// folder in `folders`, a listed file nor the name of one, but is newer
/// than every listed file (media IDs only grow), is asked for by ID until
/// it shows up: `rm`, `mv` & co. then work on the ID `upload` just printed.
async fn listed_media(
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
    // A file deleted since is in the change log: no use waiting for it.
    let now_ms = crate::config::now_unix() * 1000;
    let latest_upload = media.iter().filter_map(|m| m.date).max().unwrap_or(0);
    let from = latest_upload
        .saturating_sub(DELETED_LOOKBACK_MS)
        .max(now_ms.saturating_sub(DELETED_MAX_LOOKBACK_MS));
    if client
        .was_deleted_since(id, from, auth)
        .await
        .unwrap_or(false)
    {
        return Ok(media);
    }

    let start = std::time::Instant::now();
    let mut delay = NEW_UPLOAD_FIRST_POLL;
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
        if delay == NEW_UPLOAD_FIRST_POLL {
            eprintln!(
                "  {} isn't listed yet; waiting up to {} s in case it was just uploaded...",
                id,
                NEW_UPLOAD_WAIT.as_secs()
            );
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 3 / 2).min(NEW_UPLOAD_LONGEST_POLL);
    }
}

/// Resolve `target` to a folder or a file. Tried in order: a numeric
/// folder ID, a numeric file ID, a folder path/name, then a file path or
/// unique file name (see `find_file`).
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
    match find_file(target, folders, folder_name, media) {
        Ok(item) => {
            refuse_lone_part(item, media)?;
            Ok(Item::File(item))
        }
        // No such file, but maybe it went up in parts.
        Err(O2Error::NotFound(msg)) => {
            let first = match find_file(&part_name(target, 1), folders, folder_name, media) {
                Ok(first) => first,
                Err(O2Error::NotFound(_)) => return Err(O2Error::NotFound(msg)),
                Err(e) => return Err(e),
            };
            let base = target.rsplit('/').next().unwrap_or(target);
            Ok(Item::Parts(
                base.to_string(),
                file_parts(first, base, media)?,
            ))
        }
        Err(e) => Err(e),
    }
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
    let Lookup { media, .. } =
        media_for_target(client, auth, target, &folders, Needs::Item).await?;
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
        Item::Parts(name, parts) => {
            let ids: Vec<u64> = parts.iter().map(|p| p.id).collect();
            let path = file_path(parts[0], &folders, &folder_name);
            let path = path.strip_suffix(".001").unwrap_or(&path).to_string();
            eprintln!(
                "  {} is in {} parts: the link has them all, to download and join \
                 (`cat {}.0* > {}`, or 7-Zip)",
                name,
                ids.len(),
                name,
                name
            );
            (client.share_files(&ids, auth).await?, path, false)
        }
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
    let Lookup { media, .. } =
        media_for_target(client, auth, target, &folders, Needs::Item).await?;
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
        Item::File(_) | Item::Parts(..) => Err(O2Error::Invalid(
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
    let Lookup { media, complete } =
        media_for_target(client, auth, source, &folders, Needs::Item).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let item = match resolve_item(source, &folders, &folder_name, &media)? {
        Item::File(item) => item,
        Item::Folder(id) => {
            return move_folder(client, auth, id, destination, &folders, &folder_name).await
        }
        Item::Parts(name, parts) => {
            let dest_id = resolve_target(Some(destination.to_string()), &folders, &folder_name)?;
            let dest_path = resolve_path(dest_id, &folder_name, &folders);
            if parts[0].folderid == Some(dest_id) {
                eprintln!("{} is already in {}", name, dest_path);
                return Ok(());
            }
            let ids: Vec<u64> = parts.iter().map(|p| p.id).collect();
            let there = files_in(client, auth, Some(dest_id), &media, complete).await?;
            if name_taken(&there, &name, &ids) {
                return Err(O2Error::Conflict(format!(
                    "{} already has a file named '{}' (or parts of one)",
                    dest_path, name
                )));
            }
            let part = |p: &crate::api::MediaItem| p.name.clone().unwrap_or_default();
            update_parts(
                client,
                auth,
                &parts,
                |p| (part(p), serde_json::json!({ "folderid": dest_id })),
                |p| (part(p), serde_json::json!({ "folderid": p.folderid })),
            )
            .await?;
            eprintln!("✓ Moved {} ({} parts) → {}", name, parts.len(), dest_path);
            return Ok(());
        }
    };
    let dest_id = resolve_target(Some(destination.to_string()), &folders, &folder_name)?;
    let dest_path = resolve_path(dest_id, &folder_name, &folders);
    let from = file_path(item, &folders, &folder_name);

    if item.folderid == Some(dest_id) {
        eprintln!("{} is already in {}", from, dest_path);
        return Ok(());
    }
    if files_in(client, auth, Some(dest_id), &media, complete)
        .await?
        .iter()
        .any(|m| m.name == item.name)
    {
        return Err(O2Error::Conflict(format!(
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
        return Err(O2Error::Conflict(format!(
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
    let Lookup { media, complete } =
        media_for_target(client, auth, target, &folders, Needs::Item).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let item = match resolve_item(target, &folders, &folder_name, &media)? {
        Item::File(item) => item,
        Item::Folder(id) => {
            return rename_folder(client, auth, id, new_name, &folders, &folder_name).await
        }
        Item::Parts(name, parts) => {
            // Each part keeps its `.NNN`, which is the extension the
            // server keeps: only the name before it changes.
            if let Some(problem) = crate::api::file_name_problem(&part_name(new_name, 1)) {
                return Err(O2Error::Invalid(format!(
                    "its parts can't be named so ({}.001): {}",
                    new_name, problem
                )));
            }
            let ids: Vec<u64> = parts.iter().map(|p| p.id).collect();
            let there = files_in(client, auth, parts[0].folderid, &media, complete).await?;
            if name_taken(&there, new_name, &ids) {
                return Err(O2Error::Conflict(format!(
                    "a file named '{}' (or parts of one) already exists there",
                    new_name
                )));
            }
            let numbered = |p: &crate::api::MediaItem, base: &str| {
                let n = p
                    .name
                    .as_deref()
                    .and_then(|n| part_number(&name, n))
                    .unwrap_or(0);
                part_name(base, n)
            };
            update_parts(
                client,
                auth,
                &parts,
                |p| {
                    let new = numbered(p, new_name);
                    (numbered(p, &name), serde_json::json!({ "name": new }))
                },
                |p| {
                    let old = numbered(p, &name);
                    (numbered(p, new_name), serde_json::json!({ "name": old }))
                },
            )
            .await?;
            eprintln!("✓ Renamed {} → {} ({} parts)", name, new_name, parts.len());
            return Ok(());
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
    if files_in(client, auth, item.folderid, &media, complete)
        .await?
        .iter()
        .any(|m| m.id != item.id && m.name.as_deref() == Some(new_name))
    {
        return Err(O2Error::Conflict(format!(
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
        return Err(O2Error::Conflict(format!(
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
    let Lookup { media, complete } =
        media_for_target(client, auth, source, &folders, Needs::Item).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let item = match resolve_item(source, &folders, &folder_name, &media)? {
        Item::File(item) => item,
        Item::Folder(_) => return Err(folder_unsupported("Copying")),
        Item::Parts(base, parts) => {
            let dest_id = resolve_target(Some(destination.to_string()), &folders, &folder_name)?;
            let dest_path = resolve_path(dest_id, &folder_name, &folders);
            let name = new_name.unwrap_or(&base);
            if let Some(problem) = crate::api::file_name_problem(&part_name(name, 1)) {
                return Err(O2Error::Invalid(format!(
                    "its parts can't be named so ({}.001): {}",
                    name, problem
                )));
            }
            let there = files_in(client, auth, Some(dest_id), &media, complete).await?;
            if name_taken(&there, name, &[]) {
                return Err(O2Error::Conflict(format!(
                    "{} already has a file named '{}' (or parts of one); use --name to \
                     give the copy another name",
                    dest_path, name
                )));
            }
            let mut ids = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                match client
                    .copy_file(part.id, dest_id, Some(&part_name(name, i + 1)), auth)
                    .await
                {
                    Ok(id) => ids.push(id),
                    Err(e) => {
                        // Half a copy is no copy.
                        if !ids.is_empty() {
                            let _ = delete_files_batched(client, auth, &ids).await;
                        }
                        return Err(e);
                    }
                }
            }
            let to = if dest_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", dest_path, name)
            };
            eprintln!("✓ Copied {} ({} parts) → {}", base, parts.len(), to);
            if json {
                print_json(&serde_json::json!({
                    "parts": ids,
                    "name": name,
                    "folderid": dest_id,
                    "path": to,
                    "source_parts": parts.iter().map(|p| p.id).collect::<Vec<_>>(),
                }));
            }
            return Ok(());
        }
    };
    let dest_id = resolve_target(Some(destination.to_string()), &folders, &folder_name)?;
    let dest_path = resolve_path(dest_id, &folder_name, &folders);
    let from = file_path(item, &folders, &folder_name);
    let name = new_name.or(item.name.as_deref()).unwrap_or("?");

    if files_in(client, auth, Some(dest_id), &media, complete)
        .await?
        .iter()
        .any(|m| m.name.as_deref() == Some(name))
    {
        return Err(O2Error::Conflict(format!(
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
                return Err(O2Error::Conflict(format!(
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
        let tree = Tree::new(&folders, &folder_name, &media);
        let shown = |folder: u64| -> Vec<u64> { tree.files_in(folder).map(|m| m.id).collect() };
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
            Ok(Item::Parts(name, parts)) => format!("parts {} {}", name, parts.len()),
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
    fn test_files_in_parts() {
        let m = |id: u64, name: &str, size: u64| -> crate::api::MediaItem {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": name, "folder": 2, "size": size
            }))
            .unwrap()
        };
        let (folders, folder_name, _) = sample_tree();
        let p = DEFAULT_PART_SIZE;
        let media = vec![
            m(501, "big.iso.001", p),
            m(502, "big.iso.002", p),
            m(503, "big.iso.003", 7),
            m(504, "vol.7z.001", 100),
            m(505, "vol.7z.002", 50),
            m(506, "half.bin.001", p),
            m(507, "own", 5),
            m(508, "own.001", p),
            m(509, "own.002", 3),
        ];
        let refs: Vec<&crate::api::MediaItem> = media[..3].iter().collect();
        assert!(made_by_cli(&refs));
        assert!(made_by_cli(&[&m(501, "x.001", p - 3), &m(502, "x.002", 1)]));
        assert!(!made_by_cli(&[&media[3], &media[4]]));
        assert!(!made_by_cli(&refs[..1]));

        let kind = |t: &str| match resolve_item(t, &folders, &folder_name, &media) {
            Ok(Item::File(m)) => format!("file {}", m.id),
            Ok(Item::Parts(name, parts)) => format!("parts {} {}", name, parts.len()),
            Ok(Item::Folder(id)) => format!("folder {id}"),
            Err(e) => format!("error {}", e),
        };
        // By its own name: all its parts.
        assert_eq!(kind("/docs/big.iso"), "parts big.iso 3");
        assert_eq!(kind("/docs/vol.7z"), "parts vol.7z 2");
        // A CLI part on its own, by path: refused; by ID: fine.
        assert!(kind("/docs/big.iso.002").contains("part 2 of 3 of big.iso"));
        assert_eq!(kind("502"), "file 502");
        // Someone's own volumes, an incomplete set, and parts next to a
        // file of the same name are plain files.
        assert_eq!(kind("/docs/vol.7z.001"), "file 504");
        assert_eq!(kind("/docs/half.bin.001"), "file 506");
        assert_eq!(kind("/docs/own.001"), "file 508");
        assert!(kind("/docs/half.bin").starts_with("error"));

        assert!(name_taken(&media, "big.iso", &[]));
        assert!(!name_taken(&media, "big.iso", &[501, 502, 503]));
        assert!(name_taken(&media, "own", &[507]));
        assert!(!name_taken(&media, "other", &[]));
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
    fn test_part_names_within_limit() {
        let ok = "a".repeat(250);
        assert!(upload_pieces(&ok, 250, 100).is_ok());
        let long = "a".repeat(253); // fine whole, 257 bytes as `name.001`
        assert!(upload_pieces(&long, 50, 100).is_ok());
        let err = upload_pieces(&long, 250, 100).unwrap_err().to_string();
        assert!(err.contains("goes up in parts"), "{err}");
    }

    #[test]
    fn test_existing_parts_index() {
        let mut existing = ExistingFiles::new();
        existing.add(1, "f.002".into(), 10, 2);
        existing.add(1, "f.001".into(), 10, 1);
        existing.add(1, "f.txt".into(), 3, 3);
        existing.add(1, "f.0001".into(), 3, 4);
        existing.add(2, "f.003".into(), 10, 5);
        let parts: Vec<(usize, u64)> = existing
            .parts_of(1, "f")
            .into_iter()
            .map(|(n, found)| (n, found[0].1))
            .collect();
        assert_eq!(parts, [(1, 1), (2, 2)]);
        assert!(existing.parts_of(1, "g").is_empty());
        assert_eq!(split_part_name("a.b.010"), Some(("a.b", 10)));
        assert_eq!(split_part_name(".001"), None);
        assert_eq!(split_part_name("001"), None);
        assert_eq!(duplicate_name("a.txt"), "a (1).txt");
        assert_eq!(duplicate_name("README"), "README (1)");
        assert_eq!(duplicate_name(".bashrc"), ".bashrc (1)");
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
            folder_only: false,
            no_folder: false,
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

        let tree = Tree::new(&folders, &folder_name, &media);
        assert_eq!(tree.totals(2), (2, 12));
        assert_eq!(tree.totals(4), (1, 7));
        // The root counts everything, including files whose folder is unknown
        assert_eq!(tree.totals(1), (4, 15));
        let under = |folder: u64| -> Vec<u64> { tree.files_under(folder).map(|m| m.id).collect() };
        assert_eq!(under(2), [100, 101]);
        assert_eq!(under(1), [100, 101, 102, 103]);
        assert_eq!(tree.path(4), "/docs/deep");
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

    #[test]
    fn test_decide() {
        let opts = |update| UploadOptions {
            skip_existing: true,
            jobs: 1,
            part_size: 100,
            update,
            dry_run: false,
        };
        let mut existing = ExistingFiles::new();
        existing.insert((1, "same.txt".into()), vec![(10, 11)]);
        existing.insert((1, "dup.txt".into()), vec![(10, 21), (10, 22)]);
        existing.insert((1, "sized.txt".into()), vec![(5, 31)]);
        // A file in 3 parts of 100 + 100 + 50.
        existing.insert((1, "big.001".into()), vec![(100, 41)]);
        existing.insert((1, "big.002".into()), vec![(100, 42)]);
        existing.insert((1, "big.003".into()), vec![(50, 43)]);
        existing.insert((1, "half.001".into()), vec![(100, 51)]);
        let t = 1_600_000_000_000u64;
        let none = HashSet::new();
        let mdates: HashMap<u64, u64> = [(11, t), (21, t), (22, t), (31, t), (41, t), (51, t)]
            .into_iter()
            .collect();
        let d = |name, size, local, update| {
            decide(
                name,
                size,
                local,
                Some(1),
                &existing,
                &mdates,
                &none,
                &opts(update),
            )
        };

        assert_eq!(d("new.txt", 10, t, true), Decision::New);
        assert_eq!(
            decide("x", 1, t, None, &existing, &mdates, &none, &opts(true)),
            Decision::New
        );
        assert_eq!(d("same.txt", 10, t, true), Decision::Unchanged);
        // Within the 2 s window either way: unchanged.
        assert_eq!(d("same.txt", 10, t + 2000, true), Decision::Unchanged);
        assert_eq!(d("same.txt", 10, t - 2000, true), Decision::Unchanged);
        assert_eq!(
            d("same.txt", 10, t + 3000, true),
            Decision::Changed { old: vec![11] }
        );
        // Without --update a newer file of the same size is left alone.
        assert_eq!(d("same.txt", 10, t + 3000, false), Decision::Unchanged);
        assert_eq!(
            d("same.txt", 10, t - 3000, true),
            Decision::Older { cloud: vec![11] }
        );
        // Another size but older locally: never put over the newer copy.
        assert_eq!(
            d("sized.txt", 10, t - 3000, true),
            Decision::Older { cloud: vec![31] }
        );
        // Without --skip-existing everything is uploaded.
        let plain = UploadOptions {
            skip_existing: false,
            ..opts(false)
        };
        assert_eq!(
            decide(
                "same.txt",
                10,
                t,
                Some(1),
                &existing,
                &mdates,
                &none,
                &plain
            ),
            Decision::New
        );
        assert_eq!(
            d("sized.txt", 10, t, true),
            Decision::Changed { old: vec![31] }
        );
        // Without --update another size is left alone (not a `name (1)`
        // copy), older or not; the same size is unchanged, whatever its date.
        assert_eq!(
            d("sized.txt", 10, t, false),
            Decision::Differs {
                cloud: vec![31],
                parts: false
            }
        );
        assert_eq!(
            d("sized.txt", 10, t - 3000, false),
            Decision::Differs {
                cloud: vec![31],
                parts: false
            }
        );
        assert_eq!(d("same.txt", 10, t - 3000, false), Decision::Unchanged);
        // Duplicates: both replaced.
        assert_eq!(
            d("dup.txt", 10, t + 5000, true),
            Decision::Changed { old: vec![21, 22] }
        );
        // Parts: all there, by the first part's date.
        assert_eq!(d("big", 250, t, true), Decision::Unchanged);
        let mut changed = match d("big", 250, t + 5000, true) {
            Decision::Changed { old } => old,
            other => panic!("{:?}", other),
        };
        changed.sort();
        assert_eq!(changed, [41, 42, 43]);
        assert!(matches!(d("big", 260, t, true), Decision::Changed { .. }));
        assert_eq!(d("half", 250, t, true), Decision::Resume);

        // Crossing the part size: one file now, parts in the cloud…
        let mut old = match d("big", 90, t, true) {
            Decision::Changed { old } => old,
            other => panic!("{:?}", other),
        };
        old.sort();
        assert_eq!(old, [41, 42, 43]);
        // Crossing the part size without --update: left alone too.
        assert_eq!(
            d("big", 90, t, false),
            Decision::Differs {
                cloud: vec![41, 42, 43],
                parts: true
            }
        );
        // Parts of another size: left alone, older or not, with the parts'
        // IDs in order.
        assert_eq!(
            d("big", 260, t, false),
            Decision::Differs {
                cloud: vec![41, 42, 43],
                parts: true
            }
        );
        assert_eq!(
            d("big", 260, t - 5000, false),
            Decision::Differs {
                cloud: vec![41, 42, 43],
                parts: true
            }
        );
        // Complete parts and resuming are as with --update.
        assert_eq!(d("big", 250, t, false), Decision::Unchanged);
        assert_eq!(d("half", 250, t, false), Decision::Resume);
        // Duplicates of another size: one ID (the first), not `parts`.
        assert_eq!(
            d("dup.txt", 11, t, false),
            Decision::Differs {
                cloud: vec![21],
                parts: false
            }
        );
        // Only `name.001` in the cloud: taken for old parts…
        assert!(matches!(
            d("half", 10, t, false),
            Decision::Differs { parts: true, .. }
        ));
        // …unless `name.001` is a local file next to it: then it's that
        // file, and `half` is new (and never trashes it with --update).
        let siblings: HashSet<String> = ["half".to_string(), "half.001".to_string()].into();
        for update in [false, true] {
            assert_eq!(
                decide(
                    "half",
                    10,
                    t,
                    Some(1),
                    &existing,
                    &mdates,
                    &siblings,
                    &opts(update)
                ),
                Decision::New
            );
        }
        // …and parts now, one file in the cloud.
        existing.insert((1, "grown".into()), vec![(90, 61)]);
        let mdates: HashMap<u64, u64> = mdates.into_iter().chain([(61, t)]).collect();
        assert_eq!(
            decide(
                "grown",
                250,
                t,
                Some(1),
                &existing,
                &mdates,
                &none,
                &opts(true)
            ),
            Decision::Changed { old: vec![61] }
        );
        // Without --update: left alone, the whole file being the copy.
        assert_eq!(
            decide(
                "grown",
                250,
                t,
                Some(1),
                &existing,
                &mdates,
                &none,
                &opts(false)
            ),
            Decision::Differs {
                cloud: vec![61],
                parts: false
            }
        );
    }

    #[test]
    fn test_mtime_ms() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_600_000_000_999);
        assert_eq!(mtime_ms(t), 1_600_000_000_000);
        assert_eq!(
            mtime_ms(std::time::UNIX_EPOCH - std::time::Duration::from_secs(5)),
            0
        );
    }

    #[test]
    fn test_keep_within_o2_limits() {
        let root = Path::new("/r");
        let long_dir = "d".repeat(256);
        let long_file = format!("{}.txt", "ñ".repeat(126)); // 256 bytes
        let mut deep = String::from("/r");
        for i in 0..20 {
            deep.push_str(&format!("/l{}", i));
        }
        let mut entries: Vec<(PathBuf, bool)> = vec![
            ("/r".into(), true),
            ("/r/ok.txt".into(), false),
            (format!("/r/{}", "ñ".repeat(125) + ".txt").into(), false), // 254 bytes
            (format!("/r/{}", long_file).into(), false),
            (format!("/r/{}", long_dir).into(), true),
            (format!("/r/{}/in.txt", long_dir).into(), false),
            (format!("/r/{}", "é".repeat(255)).into(), true), // 255 chars, 510 bytes
        ];
        // `/r` becomes level 1; l0 is level 2 … l18 is level 20, l19 is 21.
        let mut path = String::from("/r");
        for i in 0..20 {
            path.push_str(&format!("/l{}", i));
            entries.push((path.clone().into(), true));
        }
        entries.push((format!("{}/deep.txt", deep).into(), false));
        let rejected = keep_within_o2_limits(&mut entries, root, 1);
        let kept: Vec<String> = entries
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect();
        assert!(kept.contains(&"/r/ok.txt".to_string()));
        assert!(kept.iter().any(|k| k.ends_with(&"é".repeat(255))));
        assert!(kept.iter().any(|k| k.ends_with("/l18")));
        assert!(!kept
            .iter()
            .any(|k| k.contains("/l19") || k.contains(&long_dir) || k.contains(&long_file)));
        let why = |end: &str| {
            rejected
                .iter()
                .find(|(p, _, _)| p.display().to_string().ends_with(end))
                .map(|(_, d, r)| (*d, r.clone()))
        };
        assert!(why(&long_file).unwrap().1.contains("256 bytes"));
        assert!(why(&long_dir).unwrap().1.contains("256 characters"));
        assert!(why("/in.txt").unwrap().1.starts_with("in "));
        assert!(why("/l19").unwrap().1.contains("levels"));
        assert!(!why("/deep.txt").unwrap().0);
        assert_eq!(rejected.iter().filter(|(_, d, _)| !d).count(), 3);
    }

    #[test]
    fn test_folder_level() {
        let f = |id, name: &str, parentid| crate::api::FolderEntry {
            id,
            name: name.into(),
            parentid,
        };
        let folders = vec![f(1, "/", 0), f(2, "a", 1), f(3, "b", 2)];
        assert_eq!(folder_level(1, &folders), 0);
        assert_eq!(folder_level(3, &folders), 2);
    }

    /// Paths longer than Windows' old 260-character limit (MAX_PATH):
    /// walked, measured and opened the way `upload-dir` does, from a
    /// relative root as users type it. Run on Windows in CI with
    /// `LongPathsEnabled` off.
    #[test]
    fn test_long_local_paths() {
        use std::io::{Read, Seek};
        let root = PathBuf::from(format!("target/longpath-test-{}", std::process::id()));
        let mut dir = root.clone();
        for i in 0..8 {
            dir.push(format!("{:02}-{}", i, "x".repeat(48)));
        }
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{}.txt", "f".repeat(60)));
        std::fs::write(&file, b"hello long path").unwrap();
        assert!(std::path::absolute(&file).unwrap().as_os_str().len() > 400);

        let mut entries = Vec::new();
        let mut skipped = 0;
        walk_dir(&root, &mut entries, &mut skipped).unwrap();
        let found: Vec<&PathBuf> = entries.iter().filter(|(_, d)| !d).map(|(p, _)| p).collect();
        assert_eq!(found, [&file]);
        assert_eq!(dir_size(&root), 15);
        assert!(local_path_in(&root, &file).ends_with(&format!("{}.txt", "f".repeat(60))));
        let meta = std::fs::metadata(found[0]).unwrap();
        assert_eq!(meta.len(), 15);
        assert!(meta.modified().is_ok());
        let mut f = std::fs::File::open(found[0]).unwrap();
        f.seek(std::io::SeekFrom::Start(6)).unwrap();
        let mut rest = String::new();
        f.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "long path");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_is_empty_image() {
        assert!(is_empty_image("IMG-20220829-WA0008.jpg", 0));
        assert!(is_empty_image("a.HEIC", 0));
        assert!(!is_empty_image("a.jpg", 1));
        assert!(!is_empty_image("a.txt", 0));
        assert!(!is_empty_image("a.heif", 0));
        assert!(!is_empty_image("jpg", 0));
    }

    #[test]
    fn test_keep_extensions() {
        let wanted =
            normalize_extensions(&[".MP4".into(), " jpg ".into(), "".into(), "*.tar.GZ".into()]);
        assert_eq!(wanted, ["mp4", "jpg", "tar.gz"]);
        let root = Path::new("/a");
        let mut entries: Vec<(PathBuf, bool)> = [
            ("/a", true),
            ("/a/x.JPG", false),
            ("/a/doc.pdf", false),
            ("/a/b", true),
            ("/a/b/c", true),
            ("/a/b/c/v.mp4", false),
            ("/a/empty", true),
            ("/a/pdfs", true),
            ("/a/pdfs/y.pdf", false),
            ("/a/noext", false),
            ("/a/jpg", false),
            ("/a/xjpg", false),
            ("/a/t", true),
            ("/a/t/b.TAR.gz", false),
            ("/a/t/c.gz", false),
        ]
        .iter()
        .map(|(p, d)| (PathBuf::from(p), *d))
        .collect();
        keep_extensions(&mut entries, root, &wanted);
        let kept: Vec<&str> = entries.iter().map(|(p, _)| p.to_str().unwrap()).collect();
        assert_eq!(
            kept,
            [
                "/a",
                "/a/x.JPG",
                "/a/b",
                "/a/b/c",
                "/a/b/c/v.mp4",
                "/a/t",
                "/a/t/b.TAR.gz"
            ]
        );
    }
}
