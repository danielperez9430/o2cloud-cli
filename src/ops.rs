use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::api::{O2Client, UploadMeta, UploadMetaResponse};
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
/// - `"24440698"` (numeric) → direct ID
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

    // Numeric → direct folder ID (must exist)
    if let Ok(id) = path.parse::<u64>() {
        return if folder_name.contains_key(&id) {
            Ok(id)
        } else {
            Err(O2Error::NotFound(format!(
                "Folder id {} not found. Try `o2cloud ls` to browse from root.",
                id
            )))
        };
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
    let metadata = std::fs::metadata(local_path).map_err(|e| {
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

    // Step 1 — Save metadata
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
    let UploadMetaResponse { id, .. } = client.upload_metadata(&meta, auth).await?;
    eprintln!("  Metadata saved (id: {})", id);

    // Step 2 — Upload bytes (streamed from disk, not buffered in memory)
    client.upload_bytes(&id, local_path, size, auth).await?;

    eprintln!("✓ Uploaded successfully (id: {})", id);
    Ok(())
}

// ------------------------------------------------------------------
// Recursive Directory Upload
// ------------------------------------------------------------------

/// Upload a local directory recursively.  Creates matching folders in
/// O2 Cloud as needed, then uploads all files.
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
    let mut entries: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
    let mut skipped_hidden = 0u32;
    walk_dir(local_dir, &mut entries, &mut skipped_hidden)?;

    // Count files for progress
    let file_count = entries.iter().filter(|(_, is_dir)| !is_dir).count();
    eprintln!(
        "→ Uploading {} ({}) — {} files, {} dirs...",
        local_dir.display(),
        format_size(dir_size(local_dir)),
        file_count,
        entries.iter().filter(|(_, is_dir)| *is_dir).count() - 1, // -1 for root
    );

    // Map: local dir path → o2 folder id
    let mut folder_map: HashMap<PathBuf, u64> = HashMap::new();
    folder_map.insert(local_dir.to_path_buf(), root_parent);

    // Create folders first (breadth-first: parent must exist before child)
    let mut dirs: Vec<_> = entries.iter().filter(|(_, is_dir)| *is_dir).collect();
    dirs.sort_by_key(|(p, _)| p.as_os_str().len()); // shortest path first = parent first

    for (dir_path, _) in &dirs {
        if *dir_path == local_dir {
            continue;
        }
        let parent_local = dir_path.parent().unwrap();
        let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(root_parent);
        let name = dir_path.file_name().unwrap().to_string_lossy().into_owned();

        // Check if folder already exists
        let existing_id = existing_folders
            .iter()
            .find(|f| f.name == name && f.parentid == parent_o2)
            .map(|f| f.id);

        let folder_id = if let Some(id) = existing_id {
            eprintln!("  📁 {}/ (exists id:{})", name, id);
            id
        } else {
            let id = client.create_folder(&name, parent_o2, auth).await?;
            eprintln!("  📁 {}/ (created id:{})", name, id);
            id
        };
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
        let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(root_parent);
        let name = file_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        attempted += 1;
        let size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

        eprintln!(
            "  [{}/{}] 📄 {} ({})",
            attempted,
            file_count,
            name,
            format_size(size)
        );

        // Upload via metadata + bytes
        let modified = std::fs::metadata(file_path)
            .ok()
            .and_then(|m| m.modified().ok());
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

        let UploadMetaResponse { id, .. } = match client.upload_metadata(&meta, auth).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  ⚠ Skipping {}: {}", name, e);
                failed += 1;
                continue;
            }
        };
        if let Err(e) = client.upload_bytes(&id, file_path, size, auth).await {
            eprintln!("  ⚠ Byte upload failed for {}: {}", name, e);
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

/// Recursively collect files and directories under `dir`, skipping any
/// entry (file or directory) whose name starts with `.` — O2 Cloud
/// rejects dotfiles, and dotdirs like `.git` should never be uploaded.
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
        if path.is_dir() {
            walk_dir(&path, entries, skipped_hidden)?;
        } else {
            entries.push((path, false));
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
            if path.is_dir() {
                total += dir_size(&path);
            } else if let Ok(meta) = std::fs::metadata(&path) {
                total += meta.len();
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
    let total_size = build_zip(local_dir, tmp_zip)?;
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

        if path.is_dir() {
            add_dir_to_zip(zip, base, &path, options)?;
        } else {
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

/// Download a file from O2 Cloud by its media ID.
///
/// If `output` is `None`, the file is saved under its real name in the
/// current directory (falling back to `{id}.bin` if the name is empty or
/// unsafe).
/// Refuses to overwrite an existing local file.
pub async fn download_file(
    client: &O2Client,
    auth: &AuthConfig,
    media_id: u64,
    output: Option<PathBuf>,
) -> Result<(), O2Error> {
    // Fetch the signed download URL (and real name) from the media listing
    let media = client.get_all_media(auth).await?;
    let item = media
        .iter()
        .find(|m| m.id == media_id)
        .ok_or_else(|| O2Error::NotFound(format!("Media {} not found", media_id)))?;
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

/// Delete a file or folder.  `target` can be a numeric ID or a path.
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
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();

    // Try numeric ID first
    if let Ok(id) = target.parse::<u64>() {
        // Is it a folder or a file?
        if folder_name.contains_key(&id) {
            let media = client.get_all_media(auth).await?;
            if !confirm_recursive_delete(id, &folders, &media, recursive, assume_yes)? {
                eprintln!("Aborted.");
                return Ok(());
            }
            return delete_folder(client, auth, id, &folders, &media, recursive).await;
        } else {
            return delete_file(client, auth, id).await;
        }
    }

    // Resolve as path
    let path = Some(target.to_string());
    match resolve_target(path, &folders, &folder_name) {
        Ok(folder_id) => {
            let media = client.get_all_media(auth).await?;
            if !confirm_recursive_delete(folder_id, &folders, &media, recursive, assume_yes)? {
                eprintln!("Aborted.");
                return Ok(());
            }
            return delete_folder(client, auth, folder_id, &folders, &media, recursive).await;
        }
        Err(_) => {
            // Not a folder — maybe it's a filename? Search media
            let media = client.get_all_media(auth).await?;
            if let Some(item) = media.iter().find(|m| m.name.as_deref() == Some(target)) {
                return delete_file(client, auth, item.id).await;
            }
            Err(O2Error::NotFound(format!(
                "'{}'. Use a numeric ID or folder path.",
                target
            )))
        }
    }
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
                "refusing to delete {} file(s) and {} subfolder(s) without a TTY; \
                 re-run with --yes (-y) to confirm non-interactively",
                files, dirs
            ),
        )));
    }

    eprint!(
        "⚠ This will permanently delete {} file(s) and {} subfolder(s). Continue? [y/N] ",
        files, dirs
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
        eprintln!(
            "📁 {}/ contains {} files and {} subfolders.",
            name,
            file_count,
            child_dirs.len()
        );
        eprintln!("Use --recursive (-r) to delete folder and all its contents.");
        return Ok(());
    }

    // Recursively delete child folders
    for child in &child_dirs {
        Box::pin(delete_folder(client, auth, child.id, folders, media, true)).await?;
    }

    // Delete files in this folder (paginated, 1000 at a time)
    let files: Vec<u64> = media
        .iter()
        .filter(|m| m.folderid == Some(folder_id))
        .map(|m| m.id)
        .collect();

    for chunk in files.chunks(1000) {
        for &mid in chunk {
            client.delete_media(mid, auth).await?;
        }
    }

    // Delete the folder itself
    client.delete_folder(folder_id, auth).await?;
    eprintln!("✓ Deleted folder {}/ (id:{})", name, folder_id);
    Ok(())
}

/// Soft-delete a file by media ID (move to trash).
async fn delete_file(client: &O2Client, auth: &AuthConfig, media_id: u64) -> Result<(), O2Error> {
    let media = client.get_all_media(auth).await?;
    let name = media
        .iter()
        .find(|m| m.id == media_id)
        .and_then(|m| m.name.as_deref())
        .unwrap_or("?");

    eprintln!("→ Deleting {} ({})...", name, media_id);
    client.delete_media(media_id, auth).await?;
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
