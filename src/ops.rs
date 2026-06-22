use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::api::{O2Client, UploadMeta, UploadMetaResponse};
use crate::cache::FileCache;
use crate::config::AuthConfig;
use crate::error::O2Error;

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
    let folder_name: HashMap<u64, String> = folders.iter().map(|f| (f.id, f.name.clone())).collect();
    let folder_parent: HashMap<u64, u64> = folders.iter().map(|f| (f.id, f.parentid)).collect();
    let folder_by_name: HashMap<String, u64> = folders.iter().map(|f| (f.name.clone(), f.id)).collect();

    // Resolve target folder
    let target = resolve_target(path, &folders, &folder_name, &folder_by_name)?;

    if tree {
        print_tree(&folders, &media, &folder_name, &folder_parent, target, "", true);
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
    let show_path = if path == "/" { "/".to_string() } else { format!("{}/", path) };
    println!("📁 {}", show_path);
    println!();

    // Subfolders
    let child_folders: Vec<_> = folders.iter().filter(|f| f.parentid == target && f.name != "/").collect();
    for f in &child_folders {
        println!("  {:<8}  📁 {}/", f.id, f.name);
    }
    if !child_folders.is_empty() {
        println!();
    }

    // Files
    let files: Vec<_> = media.iter().filter(|m| m.folderid == Some(target)).collect();
    if files.is_empty() && child_folders.is_empty() {
        println!("  (empty folder)");
        return;
    }

    for item in &files {
        let name = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(|s| format_size(s)).unwrap_or_else(|| "-".into());
        println!(
            "  {:<8}  {:>8}  {}",
            item.id, size, name,
        );
    }
}

fn print_flat_all(
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    folders: &[crate::api::FolderEntry],
) {
    println!("{:<12} {:>8}  {}", "ID", "SIZE", "PATH");
    println!("{:<12} {:>8}  {}", "──", "──", "──");

    for item in media {
        let name = item.name.as_deref().unwrap_or("?");
        let size = item.size.map(|s| format_size(s)).unwrap_or_else(|| "-".into());
        let path = item.folderid
            .map(|fid| resolve_path(fid, folder_name, folders))
            .unwrap_or_else(|| "/".into());
        let full = if path == "/" { format!("/{}", name) } else { format!("{}/{}", path, name) };

        println!("{:<12} {:>8}  {}", item.id, size, full);
    }
}

fn print_tree(
    folders: &[crate::api::FolderEntry],
    media: &[crate::api::MediaItem],
    folder_name: &HashMap<u64, String>,
    _folder_parent: &HashMap<u64, u64>,
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
    let here: Vec<_> = media.iter().filter(|m| m.folderid == Some(current)).collect();

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
        let size = item.size.map(|s| format_size(s)).unwrap_or_else(|| "-".into());
        println!("{}{}📄 {}  ({})", new_prefix, conn, fname, size);
    }

    for (_i, child) in children.iter().enumerate() {
        idx += 1;
        let last = idx == total;
        print_tree(folders, media, folder_name, _folder_parent, child.id, &new_prefix, last);
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
    _folder_by_name: &HashMap<String, u64>,
) -> Result<u64, O2Error> {
    let path = match path {
        Some(p) => p,
        None => {
            return Ok(folders.iter().find(|f| f.name == "/").map(|f| f.id).unwrap_or(0));
        }
    };

    // Numeric → direct folder ID
    if let Ok(id) = path.parse::<u64>() {
        return Ok(id);
    }

    // Absolute path → split and traverse
    let parts: Vec<&str> = if path.starts_with('/') {
        path[1..].split('/').filter(|s| !s.is_empty()).collect()
    } else {
        path.split('/').filter(|s| !s.is_empty()).collect()
    };

    if parts.is_empty() {
        return Ok(folders.iter().find(|f| f.name == "/").map(|f| f.id).unwrap_or(0));
    }

    // Build parent→children index
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    for f in folders {
        children.entry(f.parentid).or_default().push(f.id);
    }

    // Find root
    let root = folders.iter().find(|f| f.name == "/").map(|f| f.id).unwrap_or(0);

    // Traverse from root
    let mut current = root;
    for part in &parts {
        let kids = children.get(&current).ok_or_else(|| {
            O2Error::Auth(format!("Folder '{}' has no children", folder_name.get(&current).unwrap_or(&"?".into())))
        })?;
        let found = kids.iter().find_map(|kid| {
            folder_name.get(kid).and_then(|name| if name == *part { Some(*kid) } else { None })
        });
        match found {
            Some(id) => current = id,
            None => {
                // Try case-insensitive match
                let lower = part.to_lowercase();
                let fuzzy = kids.iter().find_map(|kid| {
                    folder_name.get(kid).and_then(|name| {
                        if name.to_lowercase() == lower { Some(*kid) } else { None }
                    })
                });
                match fuzzy {
                    Some(id) => current = id,
                    None => {
                        return Err(O2Error::Auth(format!(
                            "Folder '{}' not found. Try `o2cli ls` to browse from root.",
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
        if !seen.insert(cur) { break; }
        if let Some(name) = folder_name.get(&cur) {
            if name == "/" { break; }
            parts.push(name.clone());
        }
        cur = folders.iter().find(|f| f.id == cur).map(|f| f.parentid).unwrap_or(0);
        if cur == 0 { break; }
    }
    parts.reverse();
    if parts.is_empty() { "/".into() } else { format!("/{}", parts.join("/")) }
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
        O2Error::Auth(format!("Cannot read file {}: {}", local_path.display(), e))
    })?;
    let size = metadata.len();
    let name = local_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| O2Error::Auth("Invalid file name".into()))?
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
                .ok_or_else(|| O2Error::Auth("Root folder not found".into()))?
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

    // Step 2 — Upload bytes
    let data = std::fs::read(local_path).map_err(|e| {
        O2Error::Auth(format!("Cannot read file for upload: {}", e))
    })?;
    client.upload_bytes(&id, &data, auth).await?;

    // Cache the file metadata for future listings
    let mut cache = FileCache::load();
    cache.insert(
        id.parse().unwrap_or(0),
        meta.name.clone(),
        meta.size,
        meta.contenttype.clone(),
        meta.modificationdate.clone(),
        folder_id,
    )?;

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
                .ok_or_else(|| O2Error::Auth("Root folder not found".into()))?
        }
    };

    // Fetch all existing folders to check for duplicates
    let existing_folders = client.get_all_folders(auth).await?;

    // Collect all files and directories to upload
    let mut entries: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
    walk_dir(local_dir, &mut entries)?;

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
        let name = dir_path.file_name().unwrap().to_str().unwrap();

        // Check if folder already exists
        let existing_id = existing_folders
            .iter()
            .find(|f| f.name == name && f.parentid == parent_o2)
            .map(|f| f.id);

        let folder_id = if let Some(id) = existing_id {
            eprintln!("  📁 {}/ (exists id:{})", name, id);
            id
        } else {
            let id = client.create_folder(name, parent_o2, auth).await?;
            eprintln!("  📁 {}/ (created id:{})", name, id);
            id
        };
        folder_map.insert(dir_path.clone(), folder_id);
    }

    // Upload files
    let mut uploaded = 0;
    let mut skipped = 0u32;
    for (file_path, is_dir) in &entries {
        if *is_dir {
            continue;
        }
        let parent_local = file_path.parent().unwrap();
        let parent_o2 = folder_map.get(parent_local).copied().unwrap_or(root_parent);
        let name = file_path.file_name().unwrap().to_str().unwrap();

        // O2 Cloud blocks files starting with "."
        if name.starts_with('.') {
            skipped += 1;
            continue;
        }

        uploaded += 1;
        let size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

        eprintln!(
            "  [{}/{}] 📄 {} ({})",
            uploaded, file_count, name, format_size(size)
        );

        // Upload via metadata + bytes
        let modified = std::fs::metadata(file_path).ok().and_then(|m| m.modified().ok());
        let now = std::time::SystemTime::now();
        let date_str = system_time_to_compact_iso(modified.unwrap_or(now));
        let contenttype = mime_type_from_name(name);

        let meta = UploadMeta {
            contenttype: contenttype.clone(),
            creationdate: date_str.clone(),
            folderid: parent_o2,
            modificationdate: date_str,
            name: name.to_string(),
            size,
        };

        let UploadMetaResponse { id, .. } = match client.upload_metadata(&meta, auth).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  ⚠ Skipping {}: {}", name, e);
                continue;
            }
        };
        let data = match std::fs::read(file_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  ⚠ Cannot read {}: {}", file_path.display(), e);
                continue;
            }
        };
        if let Err(e) = client.upload_bytes(&id, &data, auth).await {
            eprintln!("  ⚠ Byte upload failed for {}: {}", name, e);
            continue;
        }

        // Cache
        let mut cache = FileCache::load();
        cache.insert(
            id.parse().unwrap_or(0),
            name.to_string(),
            size,
            contenttype,
            meta.modificationdate.clone(),
            parent_o2,
        )?;
    }

    if skipped > 0 {
        eprintln!("  (skipped {} dotfiles: .gitignore, .DS_Store, etc.)", skipped);
    }
    eprintln!("✓ Uploaded {} files to {}", file_count - skipped as usize, local_dir.display());
    Ok(())
}

fn walk_dir(dir: &Path, entries: &mut Vec<(PathBuf, bool)>) -> Result<(), O2Error> {
    entries.push((dir.to_path_buf(), true));
    for entry in std::fs::read_dir(dir).map_err(|e| {
        O2Error::Auth(format!("Cannot read dir {}: {}", dir.display(), e))
    })? {
        let entry = entry.map_err(|e| {
            O2Error::Auth(format!("Read error in {}: {}", dir.display(), e))
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, entries)?;
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
        return Err(O2Error::Auth(format!(
            "Not a directory: {}",
            local_dir.display()
        )));
    }

    let zip_name = format!(
        "{}.zip",
        local_dir.file_name().unwrap().to_str().unwrap()
    );
    let tmp_zip = std::env::temp_dir().join(&zip_name);

    eprintln!("→ Zipping {}...", local_dir.display());

    // Create zip
    let file = std::fs::File::create(&tmp_zip).map_err(|e| {
        O2Error::Auth(format!("Cannot create zip: {}", e))
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let base = local_dir;
    add_dir_to_zip(&mut zip, base, base, options)?;

    let total_size = std::fs::metadata(&tmp_zip).map(|m| m.len()).unwrap_or(0);
    eprintln!("  Created {} ({})", tmp_zip.display(), format_size(total_size));

    // Upload the zip
    upload_file(client, auth, &tmp_zip, folder_id).await?;

    // Cleanup
    let _ = std::fs::remove_file(&tmp_zip);
    Ok(())
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<std::fs::File>,
    base: &Path,
    dir: &Path,
    options: zip::write::SimpleFileOptions,
) -> Result<(), O2Error> {
    for entry in std::fs::read_dir(dir).map_err(|e| {
        O2Error::Auth(format!("Cannot read dir {}: {}", dir.display(), e))
    })? {
        let entry = entry.map_err(|e| {
            O2Error::Auth(format!("Read error in {}: {}", dir.display(), e))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap();

        if path.is_dir() {
            add_dir_to_zip(zip, base, &path, options)?;
        } else {
            zip.start_file(relative.to_str().unwrap(), options)
                .map_err(|e| O2Error::Auth(format!("Zip error: {}", e)))?;
            let data = std::fs::read(&path).map_err(|e| {
                O2Error::Auth(format!("Cannot read {}: {}", path.display(), e))
            })?;
            zip.write_all(&data)
                .map_err(|e| O2Error::Auth(format!("Zip write error: {}", e)))?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------
// Download
// ------------------------------------------------------------------

/// Download a file from O2 Cloud by its media ID.
pub async fn download_file(
    client: &O2Client,
    auth: &AuthConfig,
    media_id: u64,
    output_path: &Path,
) -> Result<(), O2Error> {
    // Fetch the signed download URL from the media listing
    let media = client.get_all_media(auth).await?;
    let download_url = media
        .iter()
        .find(|m| m.id == media_id)
        .and_then(|m| m.url.as_deref())
        .ok_or_else(|| O2Error::Auth(format!("Media {} not found or has no download URL", media_id)))?;

    eprintln!("→ Downloading media {}...", media_id);
    let data = client.download_file_url(download_url).await?;

    std::fs::write(output_path, &data).map_err(|e| {
        O2Error::Auth(format!(
            "Cannot write to {}: {}",
            output_path.display(),
            e
        ))
    })?;

    eprintln!(
        "✓ Downloaded {} to {}",
        format_size(data.len() as u64),
        output_path.display()
    );
    Ok(())
}

// ------------------------------------------------------------------
// Find
// ------------------------------------------------------------------

/// Search files and folders by name (case-insensitive substring).
pub async fn find(
    client: &O2Client,
    auth: &AuthConfig,
    query: &str,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let media = client.get_all_media(auth).await?;

    let folder_name: HashMap<u64, String> = folders.iter().map(|f| (f.id, f.name.clone())).collect();

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
            let folder_path = item.folderid
                .map(|fid| resolve_path(fid, &folder_name, &folders))
                .unwrap_or_else(|| "/".into());
            let full = if folder_path == "/" { format!("/{}", name) } else { format!("{}/{}", folder_path, name) };
            let size = item.size.map(|s| format_size(s)).unwrap_or_else(|| "-".into());
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
/// With `--recursive`, deletes a folder and all its contents.
pub async fn delete_target(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    recursive: bool,
) -> Result<(), O2Error> {
    let folders = client.get_all_folders(auth).await?;
    let folder_name: HashMap<u64, String> = folders.iter().map(|f| (f.id, f.name.clone())).collect();
    let folder_by_name: HashMap<String, u64> = folders.iter().map(|f| (f.name.clone(), f.id)).collect();

    // Try numeric ID first
    if let Ok(id) = target.parse::<u64>() {
        // Is it a folder or a file?
        if folder_name.contains_key(&id) {
            return delete_folder(client, auth, id, &folders, recursive).await;
        } else {
            return delete_file(client, auth, id).await;
        }
    }

    // Resolve as path
    let path = Some(target.to_string());
    match resolve_target(path, &folders, &folder_name, &folder_by_name) {
        Ok(folder_id) => {
            return delete_folder(client, auth, folder_id, &folders, recursive).await;
        }
        Err(_) => {
            // Not a folder — maybe it's a filename? Search media
            let media = client.get_all_media(auth).await?;
            if let Some(item) = media.iter().find(|m| m.name.as_deref() == Some(target)) {
                return delete_file(client, auth, item.id).await;
            }
            return Err(O2Error::Auth(format!(
                "Not found: '{}'. Use a numeric ID or folder path.",
                target
            )));
        }
    }
}

async fn delete_folder(
    client: &O2Client,
    auth: &AuthConfig,
    folder_id: u64,
    folders: &[crate::api::FolderEntry],
    recursive: bool,
) -> Result<(), O2Error> {
    let name = folders.iter().find(|f| f.id == folder_id).map(|f| f.name.as_str()).unwrap_or("?");

    // Count contents
    let media = client.get_all_media(auth).await?;
    let file_count = media.iter().filter(|m| m.folderid == Some(folder_id)).count();
    let child_dirs: Vec<_> = folders.iter().filter(|f| f.parentid == folder_id).collect();

    if !recursive && (!child_dirs.is_empty() || file_count > 0) {
        eprintln!(
            "📁 {}/ contains {} files and {} subfolders.",
            name, file_count, child_dirs.len()
        );
        eprintln!("Use --recursive (-r) to delete folder and all its contents.");
        return Ok(());
    }

    // Recursively delete child folders
    for child in &child_dirs {
        Box::pin(delete_folder(client, auth, child.id, folders, true)).await?;
    }

    // Delete files in this folder (paginated, 1000 at a time)
    let files: Vec<u64> = media.iter()
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
async fn delete_file(
    client: &O2Client,
    auth: &AuthConfig,
    media_id: u64,
) -> Result<(), O2Error> {
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

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

fn system_time_to_compact_iso(t: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
    // Compact ISO 8601: YYYYMMDDTHHMMSS
    let days_since_epoch = secs / 86400;
    // Approximate leap year handling — good enough for ~2000-2100
    let mut y = 1970;
    let mut days = days_since_epoch as i64;
    loop {
        let year_days = if is_leap(y) { 366 } else { 365 };
        if days < year_days {
            break;
        }
        days -= year_days;
        y += 1;
    }
    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 1;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        m += 1;
    }
    let d = days + 1;
    let time_secs = secs % 86400;
    let h = time_secs / 3600;
    let min = (time_secs % 3600) / 60;
    let s = time_secs % 60;
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}", y, m, d, h, min, s)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

fn mime_type_from_name(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "txt" | "md" | "rs" | "toml" | "lock" | "py" | "js" | "ts" | "html" | "css"
        | "json" | "xml" | "yaml" | "yml" | "csv" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" | "gz" | "tar" | "bz2" | "xz" | "7z" => "application/octet-stream",
        "mp4" | "mov" | "avi" | "mkv" | "webm" => "video/mp4",
        "mp3" | "wav" | "flac" | "aac" | "ogg" => "audio/mpeg",
        "php" => "application/x-httpd-php",
        _ => "application/octet-stream",
    }
    .into()
}
