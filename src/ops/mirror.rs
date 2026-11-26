//! `upload-dir --update --delete`: after uploading, what the cloud folder
//! has that the local directory no longer has goes to O2 Cloud's trash,
//! so the cloud folder mirrors the local one.
//!
//! Only inside the folder the directory is uploaded to, never above it.
//! Names are compared as O2 Cloud stores them (see `o2_name`). Kept
//! whatever the local side may still have but the upload doesn't see:
//! hidden and system names (`.git`, `Thumbs.db`…, which `upload-dir`
//! skips), the parts of a file uploaded in parts, and anything a local
//! path still exists for (a symlinked directory, a name beyond O2 Cloud's
//! limits).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{is_skipped_name, split_part_name};
use crate::api::FolderEntry;
use crate::util::o2_name;

/// How many files `--delete` moves to the trash at most unless
/// `--delete-max` says otherwise: more is likely a mistake (the wrong
/// directory, a disk not mounted).
pub const DEFAULT_DELETE_MAX: usize = 1000;

/// A cloud file, as the listing gives it.
#[derive(Clone, Debug)]
pub struct CloudFile {
    pub id: u64,
    pub folder: u64,
    pub name: String,
}

/// Something to move to the trash.
#[derive(Clone, Debug, PartialEq)]
pub struct Deletion {
    /// Its path inside the mirrored folder, e.g. `fotos/a (1).jpg`.
    pub path: String,
    pub id: u64,
    pub is_folder: bool,
    /// The files it holds (1 for a file).
    pub files: usize,
}

/// The local side, by directory (its path from the uploaded directory,
/// as O2 Cloud would name it): the files and subdirectories in it.
#[derive(Default)]
struct LocalDir {
    files: HashSet<String>,
    dirs: HashSet<String>,
}

/// What to delete from the cloud folder `top` so it mirrors `local_dir`,
/// whose files and directories `walk_dir` collected as `entries` (plus
/// those left out for O2 Cloud's limits, `left_out`). `folders` and
/// `files` are the account's (as listed before uploading: what this run
/// uploads isn't in them, and is local anyway).
pub fn plan(
    local_dir: &Path,
    entries: &[(PathBuf, bool)],
    left_out: &[PathBuf],
    top: u64,
    folders: &[FolderEntry],
    files: &[CloudFile],
) -> Vec<Deletion> {
    let mut local: HashMap<String, LocalDir> = HashMap::new();
    let all = entries
        .iter()
        .map(|(p, is_dir)| (p, *is_dir))
        .chain(left_out.iter().map(|p| (p, p.is_dir())));
    for (path, is_dir) in all {
        let Ok(rel) = path.strip_prefix(local_dir) else {
            continue;
        };
        let parts: Vec<String> = rel
            .components()
            .map(|c| o2_name(&c.as_os_str().to_string_lossy()).into_owned())
            .collect();
        let Some((name, parent)) = parts.split_last() else {
            continue;
        };
        let dir = local.entry(parent.join("/")).or_default();
        if is_dir {
            dir.dirs.insert(name.clone());
        } else {
            dir.files.insert(name.clone());
        }
    }
    let mut children: HashMap<u64, Vec<&FolderEntry>> = HashMap::new();
    for f in folders {
        if f.id != f.parentid {
            children.entry(f.parentid).or_default().push(f);
        }
    }
    let mut in_folder: HashMap<u64, Vec<&CloudFile>> = HashMap::new();
    for f in files {
        in_folder.entry(f.folder).or_default().push(f);
    }
    let mut out = Vec::new();
    visit(local_dir, top, "", &local, &children, &in_folder, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[allow(clippy::too_many_arguments)]
fn visit(
    local_dir: &Path,
    folder: u64,
    rel: &str,
    local: &HashMap<String, LocalDir>,
    children: &HashMap<u64, Vec<&FolderEntry>>,
    in_folder: &HashMap<u64, Vec<&CloudFile>>,
    out: &mut Vec<Deletion>,
) {
    let here = local.get(rel);
    let join = |name: &str| {
        if rel.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", rel, name)
        }
    };
    // Whether something named `name` here is still local, one way or
    // another, or is never compared.
    let kept = |name: &str, path: &str| {
        is_skipped_name(name) || local_dir.join(path).symlink_metadata().is_ok()
    };
    for sub in children.get(&folder).into_iter().flatten() {
        let path = join(&sub.name);
        if here.is_some_and(|d| d.dirs.contains(&sub.name)) {
            visit(local_dir, sub.id, &path, local, children, in_folder, out);
        } else if !kept(&sub.name, &path) {
            out.push(Deletion {
                files: count_files(sub.id, children, in_folder),
                path,
                id: sub.id,
                is_folder: true,
            });
        }
    }
    for file in in_folder.get(&folder).into_iter().flatten() {
        let local_file = |name: &str| here.is_some_and(|d| d.files.contains(name));
        // A part (`name.001`…) of a local file uploaded in parts.
        let part_of_local = split_part_name(&file.name).is_some_and(|(base, _)| local_file(base));
        let path = join(&file.name);
        if !local_file(&file.name) && !part_of_local && !kept(&file.name, &path) {
            out.push(Deletion {
                path,
                id: file.id,
                is_folder: false,
                files: 1,
            });
        }
    }
}

/// The files in folder `id` and below.
fn count_files(
    id: u64,
    children: &HashMap<u64, Vec<&FolderEntry>>,
    in_folder: &HashMap<u64, Vec<&CloudFile>>,
) -> usize {
    in_folder.get(&id).map_or(0, Vec::len)
        + children
            .get(&id)
            .into_iter()
            .flatten()
            .map(|f| count_files(f.id, children, in_folder))
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: u64, name: &str, parentid: u64) -> FolderEntry {
        FolderEntry {
            id,
            name: name.to_string(),
            parentid,
        }
    }

    fn file(id: u64, folder: u64, name: &str) -> CloudFile {
        CloudFile {
            id,
            folder,
            name: name.to_string(),
        }
    }

    #[test]
    fn test_plan() {
        let tmp = std::env::temp_dir().join(format!("o2-mirror-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let local = tmp.join("ALBUM");
        std::fs::create_dir_all(local.join("viaje")).unwrap();
        for f in ["a.jpg", "viaje/b.jpg", "big.iso", "Amor💙.zip"] {
            std::fs::write(local.join(f), b"x").unwrap();
        }
        let entries = vec![
            (local.clone(), true),
            (local.join("a.jpg"), false),
            (local.join("viaje"), true),
            (local.join("viaje/b.jpg"), false),
            (local.join("big.iso"), false),
            (local.join("Amor💙.zip"), false),
        ];
        // The cloud: ALBUM (10) with viaje (11), EMOJIS (12, gone
        // locally) and .git (13, never uploaded); 1 is the root.
        let folders = vec![
            folder(1, "/", 0),
            folder(10, "ALBUM", 1),
            folder(11, "viaje", 10),
            folder(12, "EMOJIS", 10),
            folder(13, ".git", 10),
            folder(20, "Otra", 1),
        ];
        let files = vec![
            file(100, 10, "a.jpg"),
            file(101, 10, "a (1).jpg"),
            file(102, 11, "b.jpg"),
            file(103, 11, "c.jpg"),
            file(104, 12, "x.zip"),
            file(105, 12, "y.zip"),
            file(106, 10, "big.iso.001"),
            file(107, 10, "big.iso.002"),
            file(108, 10, "Amor.zip"),
            file(109, 10, "Thumbs.db"),
            file(110, 20, "fuera.jpg"),
            file(111, 13, "config"),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &files);
        let paths: Vec<(&str, bool, usize)> = plan
            .iter()
            .map(|d| (d.path.as_str(), d.is_folder, d.files))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("EMOJIS", true, 2),
                ("a (1).jpg", false, 1),
                ("viaje/c.jpg", false, 1),
            ]
        );
        // Nothing outside the mirrored folder, nothing when all is local.
        assert!(plan.iter().all(|d| d.id != 110 && d.id != 20));
        let same_folders = vec![
            folder(1, "/", 0),
            folder(10, "ALBUM", 1),
            folder(11, "viaje", 10),
        ];
        let same_files = vec![
            file(100, 10, "a.jpg"),
            file(102, 11, "b.jpg"),
            file(106, 10, "big.iso.001"),
            file(108, 10, "Amor.zip"),
        ];
        let all_local = super::plan(&local, &entries, &[], 10, &same_folders, &same_files);
        assert!(all_local.is_empty(), "{all_local:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_kept_when_still_on_disk() {
        // A local directory the walk didn't list (a symlink, say) keeps
        // its cloud copy.
        let tmp = std::env::temp_dir().join(format!("o2-mirror-test2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let local = tmp.join("ALBUM");
        std::fs::create_dir_all(local.join("enlazada")).unwrap();
        let entries = vec![(local.clone(), true)];
        let folders = vec![folder(10, "ALBUM", 1), folder(11, "enlazada", 10)];
        let plan = plan(&local, &entries, &[], 10, &folders, &[file(1, 11, "z.jpg")]);
        assert!(plan.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
