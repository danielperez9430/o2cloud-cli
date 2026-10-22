use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::api::{O2Client, UploadMeta};
use crate::config::AuthConfig;
use crate::error::O2Error;
use crate::util::{format_size, mime_type_from_name, system_time_to_compact_iso};

// ------------------------------------------------------------------
// List
// ------------------------------------------------------------------

/// List files with multiple display modes:
/// - Default: root folder contents (direct children)
/// - `path`: folder path (e.g. `/DMHAIR/app`) or ID (e.g. `24440698`)
/// - `all`: all files flat list
/// - `tree`: hierarchical tree view
pub async fn list_folder(
    client: &O2Client,
    auth: &AuthConfig,
    path: Option<String>,
    all: bool,
    tree: bool,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = client.get_all_media(auth).await?;

    if media.is_empty() && folders.is_empty() {
        println!("(empty)");
        return Ok(());
    }

    // Build lookup tables
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    // Resolve target folder
    let target = resolve_target(path, &folders, &folder_name)?;

    if tree {
        print_tree(&folders, &media, &folder_name, target, "", true);
    } else if all {
        print_flat_all(&media, &folder_name, &folders);
    } else {
        print_folder_contents(&folders, &media, &folder_name, target);
    }

    Ok(())
}

// ------------------------------------------------------------------
// Display helpers
// ------------------------------------------------------------------

fn print_folder_contents(
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    target: u64,
) {
    let path = resolve_path(target, folder_name, folders);

    // Breadcrumb
    let show_path = if path == "/" {
        "/".to_string()
    } else {
        format!("{}/", path)
    };
    println!("📁 {}", show_path);
    println!();

    // Subfolders
    let child_folders: Vec<_> = folders
        .iter()
        .filter(|f| f.parentid == target && f.name != "/")
        .collect();
    for f in &child_folders {
        println!("  {:<8}  📁 {}/", f.id, f.name);
    }
    if !child_folders.is_empty() {
        println!();
    }

    // Files
    let files: Vec<_> = media
        .iter()
        .filter(|m| m.folderid == Some(target))
        .collect();
    if files.is_empty() && child_folders.is_empty() {
        println!("  (empty folder)");
        return;
    }

    for item in &files {
        let name = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
        println!("  {:<8}  {:>8}  {}", item.id, size, name,);
    }
}

fn print_flat_all(
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    folders: &[crate::api::FolderEntry],
) {
    println!("{:<12} {:>8}  PATH", "ID", "SIZE");
    println!("{:<12} {:>8}  ──", "──", "──");

    for item in media {
        let name = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
        let path = item
            .folderid
            .map(|fid| resolve_path(fid, folder_name, folders))
            .unwrap_or_else(|| "/".into());
        let full = if path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", path, name)
        };

        println!("{:<12} {:>8}  {}", item.id, size, full);
    }
}

fn print_tree(
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    current: u64,
    prefix: &str,
    is_last: bool,
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
    let here: Vec<_> = media
        .iter()
        .filter(|m| m.folderid == Some(current))
        .collect();

    // Child folders (excluding root which has parentid=0)
    let children: Vec<_> = folders
        .iter()
        .filter(|f| f.parentid == current && f.name != "/" && f.parentid != 0)
        .collect();

    let total = here.len() + children.len();
    let mut idx = 0;

    for item in &here {
        idx += 1;
        let last = idx == total;
        let conn = if last { "└── " } else { "├── " };
        let fname = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
        println!("{}{}📄 {}  ({})", new_prefix, conn, fname, size);
    }

    for child in children.iter() {
        idx += 1;
        let last = idx == total;
        print_tree(folders, media, folder_name, child.id, &new_prefix, last);
    }
}

/// Resolve a user-supplied path string to a folder ID.
///
/// Supported formats:
/// - `None` → root folder
/// - `"24440698"` (numeric) → direct ID, or a root folder with that name
///   (e.g. `2024`) if no folder has that ID
/// - `"/DMHAIR/app"` (absolute path) → traverse tree
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

/// Upload a local file to O2 Cloud.
pub async fn upload_file(
    client: &O2Client,
    auth: &AuthConfig,
    local_path: &Path,
    folder_id: Option<u64>,
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
        .ok_or_else(|| {
            O2Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid file name",
            ))
        })?
        .to_string();

    let modified = metadata.modified().ok();
    let now = std::time::SystemTime::now();
    let date_str = system_time_to_compact_iso(modified.unwrap_or(now));

    let contenttype = mime_type_from_name(&name);

    // Resolve folder ID
    let folder_id = match folder_id {
        Some(id) => id,
        None => {
            let folders = client.get_all_folders(auth).await?;
            folders
                .iter()
                .find(|f| f.name == "/")
                .map(|f| f.id)
                .ok_or_else(|| O2Error::NotFound("Root folder not found".into()))?
        }
    };

    let meta = UploadMeta {
        contenttype,
        creationdate: date_str.clone(),
        folderid: folder_id,
        modificationdate: date_str,
        name,
        size,
    };

    eprintln!(
        "→ Uploading {} ({})...",
        local_path.display(),
        format_size(size)
    );
    // Metadata and contents in one request, streamed from disk
    let id = client.upload_file(&meta, local_path, auth).await?;

    eprintln!("✓ Uploaded successfully (id: {})", id);
    Ok(())
}

// ------------------------------------------------------------------
// Recursive Directory Upload
// ------------------------------------------------------------------

/// Upload a local directory recursively.  The directory is recreated as a
/// folder of the same name inside the target (reusing it if it exists),
/// with matching subfolders, then all files are uploaded.
pub async fn upload_dir(
    client: &O2Client,
    auth: &AuthConfig,
    local_dir: &Path,
    parent_folder_id: Option<u64>,
) -> Result<(), O2Error> {
    // Resolve target parent folder
    let root_parent = match parent_folder_id {
        Some(id) => id,
        None => {
            let folders = client.get_all_folders(auth).await?;
            folders
                .iter()
                .find(|f| f.name == "/")
                .map(|f| f.id)
                .ok_or_else(|| O2Error::NotFound("Root folder not found".into()))?
        }
    };

    // Fetch all existing folders to check for duplicates
    let existing_folders = client.get_all_folders(auth).await?;

    // Collect all files and directories to upload, skipping hidden entries
    // (dotfiles/dotdirs like .git, .DS_Store) which O2 Cloud rejects.
    // The walk is synchronous filesystem work, so keep it off the runtime.
    let dir = local_dir.to_path_buf();
    let (entries, skipped_hidden, total_size) = spawn_blocking_io(move || {
        let mut entries: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
        let mut skipped_hidden = 0u32;
        walk_dir(&dir, &mut entries, &mut skipped_hidden)?;
        Ok((entries, skipped_hidden, dir_size(&dir)))
    })
    .await?;

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

    // Upload files
    let mut attempted = 0usize;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    for (file_path, is_dir) in &entries {
        if *is_dir {
            continue;
        }
        let parent_local = file_path.parent().unwrap();
        let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(top_id);
        let name = file_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        attempted += 1;
        let file_meta = tokio::fs::metadata(file_path).await.ok();
        let size = file_meta.as_ref().map(|m| m.len()).unwrap_or(0);

        eprintln!(
            "  [{}/{}] 📄 {} ({})",
            attempted,
            file_count,
            name,
            format_size(size)
        );

        let modified = file_meta.and_then(|m| m.modified().ok());
        let now = std::time::SystemTime::now();
        let date_str = system_time_to_compact_iso(modified.unwrap_or(now));
        let contenttype = mime_type_from_name(&name);

        let meta = UploadMeta {
            contenttype: contenttype.clone(),
            creationdate: date_str.clone(),
            folderid: parent_o2,
            modificationdate: date_str,
            name: name.clone(),
            size,
        };

        if let Err(e) = client.upload_file(&meta, file_path, auth).await {
            eprintln!("  ⚠ Upload failed for {}: {}", name, e);
            failed += 1;
            continue;
        }

        succeeded += 1;
    }

    if skipped_hidden > 0 {
        eprintln!(
            "  (skipped {} hidden files/dirs: .git, .DS_Store, etc.)",
            skipped_hidden
        );
    }
    if failed > 0 {
        eprintln!("⚠ {} file(s) failed to upload", failed);
    }
    eprintln!("✓ Uploaded {} files to {}", succeeded, local_dir.display());

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

/// Recursively collect files and directories under `dir`, skipping any
/// entry (file or directory) whose name starts with `.` — O2 Cloud
/// rejects dotfiles, and dotdirs like `.git` should never be uploaded.
/// Symlinked files are followed; symlinked directories are skipped.
fn walk_dir(
    dir: &Path,
    entries: &mut Vec<(PathBuf, bool)>,
    skipped_hidden: &mut u32,
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
        let is_hidden = path
            .file_name()
            .map(|n| n.to_string_lossy().starts_with('.'))
            .unwrap_or(false);
        if is_hidden {
            *skipped_hidden += 1;
            continue;
        }
        match entry_kind(&entry) {
            EntryKind::Dir => walk_dir(&path, entries, skipped_hidden)?,
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
            let is_hidden = path
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false);
            if is_hidden {
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
) -> Result<(), O2Error> {
    if !local_dir.is_dir() {
        return Err(O2Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Not a directory: {}", local_dir.display()),
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
    let result = zip_and_upload(client, auth, local_dir, &tmp_zip, folder_id).await;
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
) -> Result<(), O2Error> {
    eprintln!("→ Zipping {}...", local_dir.display());
    let (dir, zip_path) = (local_dir.to_path_buf(), tmp_zip.to_path_buf());
    let total_size = spawn_blocking_io(move || build_zip(&dir, &zip_path)).await?;
    eprintln!(
        "  Created {} ({})",
        tmp_zip.display(),
        format_size(total_size)
    );

    upload_file(client, auth, tmp_zip, folder_id).await
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
) -> Result<(), O2Error> {
    // Fetch the signed download URL (and real name) from the media listing
    let media = client.get_all_media(auth).await?;
    let by_id = target
        .parse::<u64>()
        .ok()
        .and_then(|id| media.iter().find(|m| m.id == id));
    let item = match by_id {
        Some(item) => item,
        // Not a known ID: resolve as a file path or unique file name.
        None => {
            let folders = client.get_all_folders(auth).await?;
            let folder_name: HashMap<u64, String> =
                folders.iter().map(|f| (f.id, f.name.clone())).collect();
            find_file(target, &folders, &folder_name, &media)?
        }
    };
    let media_id = item.id;
    let download_url = item
        .url
        .as_deref()
        .ok_or_else(|| O2Error::NotFound(format!("Media {} has no download URL", media_id)))?;

    let output_path = match output {
        Some(p) => p,
        None => match item.name.as_deref().and_then(safe_local_file_name) {
            Some(name) => PathBuf::from(name),
            None => PathBuf::from(format!("{}.bin", media_id)),
        },
    };

    if output_path.exists() {
        return Err(O2Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "Refusing to overwrite existing file: {}",
                output_path.display()
            ),
        )));
    }

    eprintln!("→ Downloading media {}...", media_id);
    let bytes_written = client.download_to_path(download_url, &output_path).await?;

    eprintln!(
        "✓ Downloaded {} to {}",
        format_size(bytes_written),
        output_path.display()
    );
    Ok(())
}

// ------------------------------------------------------------------
// Find
// ------------------------------------------------------------------

/// Search files and folders by name (case-insensitive substring).
pub async fn find(client: &O2Client, auth: &AuthConfig, query: &str) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = client.get_all_media(auth).await?;

    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    let q = query.to_lowercase();
    let mut found = 0u32;

    // Folders
    for f in &folders {
        if f.name.to_lowercase().contains(&q) {
            let path = resolve_path(f.id, &folder_name, &folders);
            println!("📁 {:<12}  {}/", f.id, path);
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
            let size = item.size.map(format_size).unwrap_or_else(|| "-".into());
            println!("📄 {:<12} {:>8}  {}", item.id, size, full);
            found += 1;
        }
    }

    if found == 0 {
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
    let media = client.get_all_media(auth).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    // Numeric ID: a folder or a file. If it matches neither, fall through
    // and treat it as a name (e.g. a folder called `2024`).
    if let Ok(id) = target.parse::<u64>() {
        if folder_name.contains_key(&id) {
            return delete_folder_checked(
                client,
                auth,
                id,
                &folders,
                &folder_name,
                &media,
                recursive,
                assume_yes,
            )
            .await;
        }
        if let Some(item) = media.iter().find(|m| m.id == id) {
            return delete_file(client, auth, item).await;
        }
    }

    if let Ok(folder_id) = resolve_target(Some(target.to_string()), &folders, &folder_name) {
        return delete_folder_checked(
            client,
            auth,
            folder_id,
            &folders,
            &folder_name,
            &media,
            recursive,
            assume_yes,
        )
        .await;
    }

    let item = find_file(target, &folders, &folder_name, &media)?;
    delete_file(client, auth, item).await
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
                .map(|m| {
                    let dir = m
                        .folderid
                        .map(|fid| resolve_path(fid, folder_name, folders))
                        .unwrap_or_else(|| "/".into());
                    let name = m.name.as_deref().unwrap_or("?");
                    let path = if dir == "/" {
                        format!("/{}", name)
                    } else {
                        format!("{}/{}", dir, name)
                    };
                    format!("  {:<12}  {}", m.id, path)
                })
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
        return Err(O2Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to delete the root folder",
        )));
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
        return Err(O2Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "refusing to delete {}/ ({} file(s), {} subfolder(s)) without a TTY; \
                 re-run with --yes (-y) to confirm non-interactively",
                path, files, dirs
            ),
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
        return Err(O2Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{}/ contains {} files and {} subfolders; use --recursive (-r) \
                 to delete the folder and all its contents",
                name,
                file_count,
                child_dirs.len()
            ),
        )));
    }

    // Recursively delete child folders
    for child in &child_dirs {
        Box::pin(delete_folder(client, auth, child.id, folders, media, true)).await?;
    }

    // Delete files in this folder, in batches of up to 1000 IDs
    // (the server's `folders.softdelete-updates-limit`)
    let files: Vec<u64> = media
        .iter()
        .filter(|m| m.folderid == Some(folder_id))
        .map(|m| m.id)
        .collect();

    for chunk in files.chunks(1000) {
        client.delete_files(chunk, auth).await?;
    }

    // Delete the folder itself
    client.delete_folder(folder_id, auth).await?;
    eprintln!("✓ Deleted folder {}/ (id:{})", name, folder_id);
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
}
