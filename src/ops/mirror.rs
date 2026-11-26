//! `upload-dir --update --delete`: after uploading, what the cloud folder
//! has that the local directory no longer has goes to O2 Cloud's trash,
//! so the cloud folder mirrors the local one.
//!
//! Only inside the folder the directory is uploaded to, never above it.
//! Names are compared as O2 Cloud stores them (see `o2_name`). It errs on
//! the side of keeping: never planned are hidden and system names (`.git`,
//! `Thumbs.db`…, which `upload-dir` skips), the parts of a file uploaded in
//! parts, a replacement's temporary copy (`*.o2-replacing-<id>*`), anything
//! a local path still exists for (a symlinked directory, a name beyond O2
//! Cloud's limits), a name that only differs from a local one in case,
//! spaces or emoji, and what a local directory that is empty holds (a disk
//! not mounted there would otherwise empty its cloud folder). Two local
//! names that O2 Cloud would store as one stop the plan: which cloud copy
//! is which can't be told.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{is_skipped_name, split_part_name};
use crate::api::FolderEntry;
use crate::util::o2_name;

/// How many files and folders `--delete` moves to the trash at most unless
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Deletion {
    /// Its path inside the mirrored folder, e.g. `fotos/a (1).jpg`.
    pub path: String,
    pub id: u64,
    pub is_folder: bool,
    /// The files it holds (1 for a file).
    pub files: usize,
    /// The folders it is (itself and those inside; 0 for a file).
    pub folders: usize,
}

/// What `plan` found: what to delete, and what it kept that looked like
/// something to delete (said to the user).
#[derive(Debug, Default)]
pub struct Plan {
    pub deletions: Vec<Deletion>,
    pub kept: Vec<String>,
    /// The files in the mirrored cloud folder, in all.
    pub cloud_files: usize,
}

impl Plan {
    /// Files and folders it would move to the trash, counted as the
    /// `--delete-max` limit counts them.
    pub fn items(&self) -> usize {
        self.deletions.iter().map(|d| d.files + d.folders).sum()
    }
}

/// The local side, by directory (its path from the uploaded directory,
/// as O2 Cloud would name it): its files and subdirectories, by the name
/// O2 Cloud would give them, with the local names behind each.
#[derive(Default)]
struct LocalDir {
    files: HashMap<String, Vec<String>>,
    dirs: HashMap<String, Vec<String>>,
}

impl LocalDir {
    /// The names here as `loose` makes them, to tell a cloud name that is
    /// almost a local one.
    fn loose_names(&self) -> HashSet<String> {
        self.files
            .keys()
            .chain(self.dirs.keys())
            .map(|n| loose(n))
            .collect()
    }
}

/// A name as O2 Cloud would store it; one made only of characters it
/// drops keeps them (it couldn't be stored, so it can't match).
fn stored(name: &str) -> String {
    let s = o2_name(name);
    if s.is_empty() {
        name.to_string()
    } else {
        s.into_owned()
    }
}

/// `name` lowercased, without characters O2 Cloud drops and with runs of
/// whitespace as one space, trimmed: two names equal so are too close to
/// tell apart safely.
fn loose(name: &str) -> String {
    o2_name(name)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `name` is a replacement's temporary copy (`ops::aside_name`):
/// the old version, still the only one if a replacement stopped halfway.
fn is_aside_copy(name: &str) -> bool {
    name.contains(".o2-replacing-") || name.contains("~o2-replacing-")
}

/// What to delete from the cloud folder `top` so it mirrors `local_dir`,
/// whose files and directories `walk_dir` collected as `entries` (plus
/// those left out for O2 Cloud's limits, `left_out`: path and whether it
/// is a directory). `folders` and `files` are the account's. An error says
/// why it can't be planned safely.
pub fn plan(
    local_dir: &Path,
    entries: &[(PathBuf, bool)],
    left_out: &[(PathBuf, bool)],
    top: u64,
    folders: &[FolderEntry],
    files: &[CloudFile],
) -> Result<Plan, String> {
    let mut local: HashMap<String, LocalDir> = HashMap::new();
    // Each local directory by its key, as it is on disk: names are checked
    // on disk under it (not under the cloud's names, which lack emoji).
    let mut real: HashMap<String, PathBuf> = HashMap::new();
    real.insert(String::new(), local_dir.to_path_buf());
    // The directories with a file somewhere below them.
    let mut with_files: HashSet<String> = HashSet::new();
    for (path, is_dir) in entries.iter().chain(left_out) {
        let rel = path
            .strip_prefix(local_dir)
            .map_err(|_| format!("{} is not inside {}", path.display(), local_dir.display()))?;
        let raw: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some((name, parent)) = raw.split_last() else {
            continue; // the directory itself
        };
        let keys: Vec<String> = parent.iter().map(|c| stored(c)).collect();
        let key = keys.join("/");
        if *is_dir {
            let own = if key.is_empty() {
                stored(name)
            } else {
                format!("{}/{}", key, stored(name))
            };
            real.insert(own, path.clone());
        } else {
            for n in 0..=keys.len() {
                with_files.insert(keys[..n].join("/"));
            }
        }
        let dir = local.entry(key).or_default();
        let names = if *is_dir {
            &mut dir.dirs
        } else {
            &mut dir.files
        };
        names.entry(stored(name)).or_default().push(name.clone());
    }
    // Two local names O2 Cloud stores as one: its `name (1)` is one of
    // them, and can't be told from a stray copy.
    let mut clashes: Vec<String> = Vec::new();
    for (dir, d) in &local {
        for names in d.files.values().chain(d.dirs.values()) {
            if names.len() > 1 {
                let mut shown: Vec<String> = names
                    .iter()
                    .map(|n| {
                        if dir.is_empty() {
                            n.clone()
                        } else {
                            format!("{}/{}", dir, n)
                        }
                    })
                    .collect();
                shown.sort();
                clashes.push(shown.join(" and "));
            }
        }
    }
    if !clashes.is_empty() {
        clashes.sort();
        return Err(format!(
            "O2 Cloud stores these names as one (it drops emoji and similar characters), \
             so --delete can't tell their copies apart: {}. Rename them so they differ \
             in more than emoji, then run again",
            clashes.join("; ")
        ));
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
    let mut plan = Plan {
        cloud_files: count(top, &children, &in_folder).0,
        ..Plan::default()
    };
    let tree = Tree {
        local: &local,
        real: &real,
        with_files: &with_files,
        children: &children,
        in_folder: &in_folder,
    };
    tree.visit(top, "", &mut plan);
    plan.deletions.sort_by(|a, b| a.path.cmp(&b.path));
    plan.kept.sort();
    Ok(plan)
}

struct Tree<'a> {
    local: &'a HashMap<String, LocalDir>,
    real: &'a HashMap<String, PathBuf>,
    with_files: &'a HashSet<String>,
    children: &'a HashMap<u64, Vec<&'a FolderEntry>>,
    in_folder: &'a HashMap<u64, Vec<&'a CloudFile>>,
}

/// Why something not local by name is kept anyway.
enum Keep {
    /// Never deleted, nothing to say (hidden or system names).
    Quietly,
    /// Kept, and said why.
    Saying(String),
}

impl Tree<'_> {
    fn visit(&self, folder: u64, rel: &str, plan: &mut Plan) {
        let join = |name: &str| {
            if rel.is_empty() {
                name.to_string()
            } else {
                format!("{}/{}", rel, name)
            }
        };
        let subfolders = self.children.get(&folder).map_or(&[][..], Vec::as_slice);
        let files = self.in_folder.get(&folder).map_or(&[][..], Vec::as_slice);
        let has_content = !subfolders.is_empty() || !files.is_empty();
        // A local directory with no file anywhere below it (not the top
        // one: that is refused before), perhaps a disk not mounted or a
        // copy that didn't finish: its cloud contents stay.
        if !rel.is_empty() && !self.with_files.contains(rel) {
            if has_content {
                let (n, _) = count(folder, self.children, self.in_folder);
                plan.kept.push(format!(
                    "{}/: no files in it locally, so its {} file(s) in the cloud were kept \
                     (delete the local folder itself to have it removed)",
                    rel, n
                ));
            }
            return;
        }
        let empty = LocalDir::default();
        let here = self.local.get(rel).unwrap_or(&empty);
        let loose_here = here.loose_names();
        let real_dir = self.real.get(rel);
        // Kept although not local by name, or `None` to delete.
        let keep = |name: &str, path: &str| -> Option<Keep> {
            if is_skipped_name(name) {
                return Some(Keep::Quietly);
            }
            if is_aside_copy(name) {
                return Some(Keep::Saying(format!(
                    "{}: kept, a replacement's copy of the old version",
                    path
                )));
            }
            // On disk under the real local directory, one component at
            // a time (a `\\?\` path takes no `/`).
            if let Some(dir) = real_dir {
                match dir.join(name).symlink_metadata() {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Some(Keep::Saying(format!(
                            "{}: kept, couldn't check it locally ({})",
                            path, e
                        )))
                    }
                    Ok(_) => {
                        return Some(Keep::Saying(format!(
                            "{}: kept, something of that name is on the local disk (a \
                             link, another case, a name beyond O2 Cloud's limits)",
                            path
                        )))
                    }
                }
            }
            if loose_here.contains(&loose(name)) {
                return Some(Keep::Saying(format!(
                    "{}: kept, its name is almost that of a local one (case, spaces or emoji)",
                    path
                )));
            }
            None
        };
        for sub in subfolders {
            let path = join(&sub.name);
            if here.dirs.contains_key(&sub.name) {
                self.visit(sub.id, &path, plan);
                continue;
            }
            match keep(&sub.name, &path) {
                Some(Keep::Quietly) => {}
                Some(Keep::Saying(why)) => plan.kept.push(why),
                None => {
                    // A folder holding what is never deleted (hidden or
                    // system names, a replacement's copy) stays whole.
                    if let Some(inside) = self.protected_inside(sub.id) {
                        plan.kept.push(format!(
                            "{}/: kept, it holds {}; delete it yourself if you're sure",
                            path, inside
                        ));
                        continue;
                    }
                    let (files, folders) = count(sub.id, self.children, self.in_folder);
                    plan.deletions.push(Deletion {
                        path,
                        id: sub.id,
                        is_folder: true,
                        files,
                        folders: folders + 1,
                    });
                }
            }
        }
        for file in files {
            let local_file = |name: &str| here.files.contains_key(name);
            // A part (`name.001`…) of a local file uploaded in parts.
            let part_of_local =
                split_part_name(&file.name).is_some_and(|(base, _)| local_file(base));
            if local_file(&file.name) || part_of_local {
                continue;
            }
            let path = join(&file.name);
            match keep(&file.name, &path) {
                Some(Keep::Quietly) => {}
                Some(Keep::Saying(why)) => plan.kept.push(why),
                None => plan.deletions.push(Deletion {
                    path,
                    id: file.id,
                    is_folder: false,
                    files: 1,
                    folders: 0,
                }),
            }
        }
    }

    /// A name in folder `id` or below that is never deleted, if any.
    fn protected_inside(&self, id: u64) -> Option<String> {
        for f in self.in_folder.get(&id).into_iter().flatten() {
            if is_skipped_name(&f.name) || is_aside_copy(&f.name) {
                return Some(f.name.clone());
            }
        }
        for sub in self.children.get(&id).into_iter().flatten() {
            if is_skipped_name(&sub.name) {
                return Some(format!("{}/", sub.name));
            }
            if let Some(found) = self.protected_inside(sub.id) {
                return Some(found);
            }
        }
        None
    }
}

/// The files and the folders in folder `id` and below (not counting it).
fn count(
    id: u64,
    children: &HashMap<u64, Vec<&FolderEntry>>,
    in_folder: &HashMap<u64, Vec<&CloudFile>>,
) -> (usize, usize) {
    let mut files = in_folder.get(&id).map_or(0, Vec::len);
    let mut folders = 0;
    for f in children.get(&id).into_iter().flatten() {
        let (fi, fo) = count(f.id, children, in_folder);
        files += fi;
        folders += fo + 1;
    }
    (files, folders)
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

    /// A local tree under a fresh temporary directory: `ALBUM` with these
    /// directories and files.
    fn local_tree(tag: &str, files: &[&str], dirs: &[&str]) -> (PathBuf, Vec<(PathBuf, bool)>) {
        let tmp = std::env::temp_dir().join(format!("o2-mirror-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let local = tmp.join("ALBUM");
        std::fs::create_dir_all(&local).unwrap();
        let mut entries = vec![(local.clone(), true)];
        for d in dirs {
            std::fs::create_dir_all(local.join(d)).unwrap();
            entries.push((local.join(d), true));
        }
        for f in files {
            std::fs::write(local.join(f), b"x").unwrap();
            entries.push((local.join(f), false));
        }
        (local, entries)
    }

    fn paths(plan: &Plan) -> Vec<(&str, usize, usize)> {
        plan.deletions
            .iter()
            .map(|d| (d.path.as_str(), d.files, d.folders))
            .collect()
    }

    #[test]
    fn test_plan() {
        let (local, entries) = local_tree(
            "plan",
            &["a.jpg", "viaje/b.jpg", "big.iso", "Amor💙.zip"],
            &["viaje"],
        );
        // The cloud: ALBUM (10) with viaje (11), EMOJIS (12, gone
        // locally, with a subfolder 14) and .git (13, never uploaded); 1
        // is the root, 20 another folder.
        let folders = vec![
            folder(1, "/", 0),
            folder(10, "ALBUM", 1),
            folder(11, "viaje", 10),
            folder(12, "EMOJIS", 10),
            folder(13, ".git", 10),
            folder(14, "sub", 12),
            folder(20, "Otra", 1),
        ];
        let files = vec![
            file(100, 10, "a.jpg"),
            file(101, 10, "a (1).jpg"),
            file(102, 11, "b.jpg"),
            file(103, 11, "c.jpg"),
            file(104, 12, "x.zip"),
            file(105, 14, "y.zip"),
            file(106, 10, "big.iso.001"),
            file(107, 10, "big.iso.002"),
            file(108, 10, "Amor.zip"),
            file(109, 10, "Thumbs.db"),
            file(110, 20, "fuera.jpg"),
            file(111, 13, "config"),
            file(112, 10, "a.o2-replacing-100.jpg"),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert_eq!(
            paths(&plan),
            vec![("EMOJIS", 2, 2), ("a (1).jpg", 1, 0), ("viaje/c.jpg", 1, 0)]
        );
        assert_eq!(plan.items(), 6);
        assert_eq!(plan.cloud_files, 12);
        // Nothing outside the mirrored folder.
        assert!(plan.deletions.iter().all(|d| d.id != 110 && d.id != 20));
        // All local: nothing.
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
        let all_local = super::plan(&local, &entries, &[], 10, &same_folders, &same_files).unwrap();
        assert!(all_local.deletions.is_empty(), "{:?}", all_local.deletions);
    }

    #[test]
    fn test_clashing_names_stop_the_plan() {
        // `a😀.jpg` and `a.jpg` are both `a.jpg` in O2 Cloud: its
        // `a (1).jpg` is one of them, not a stray copy.
        let (local, entries) = local_tree("clash", &["a😀.jpg", "a.jpg"], &[]);
        let folders = vec![folder(10, "ALBUM", 1)];
        let files = vec![file(1, 10, "a.jpg"), file(2, 10, "a (1).jpg")];
        let err = plan(&local, &entries, &[], 10, &folders, &files).unwrap_err();
        assert!(err.contains("a.jpg and a😀.jpg"), "{err}");
    }

    #[test]
    fn test_empty_local_folder_keeps_the_cloud_one() {
        // `2019` is there locally but empty (a disk not mounted?): its
        // cloud files stay.
        let (local, entries) = local_tree("empty", &["a.jpg"], &["2019"]);
        let folders = vec![folder(10, "ALBUM", 1), folder(11, "2019", 10)];
        let files = vec![
            file(1, 10, "a.jpg"),
            file(2, 11, "x.jpg"),
            file(3, 11, "y.jpg"),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert!(plan.deletions.is_empty());
        assert!(
            plan.kept[0].starts_with("2019/: no files in it locally"),
            "{:?}",
            plan.kept
        );
    }

    #[test]
    fn test_near_names_are_kept() {
        // Differing only in case, spaces or emoji from a local name: kept
        // and said (on a case-sensitive disk too).
        let (local, entries) = local_tree("near", &["Foto.jpg", "viaje 2024.txt"], &[]);
        let folders = vec![folder(10, "ALBUM", 1)];
        let files = vec![
            file(1, 10, "Foto.jpg"),
            file(2, 10, "foto.JPG"),
            file(3, 10, "viaje  2024.txt"),
            file(4, 10, "viaje 2024.txt"),
            file(5, 10, "otra.txt"),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert_eq!(paths(&plan), vec![("otra.txt", 1, 0)]);
        // Said for the spaces; for the case too where the disk tells
        // `foto.JPG` from `Foto.jpg` (on others it's just kept).
        assert!(
            plan.kept.iter().any(|k| k.starts_with("viaje  2024.txt")),
            "{:?}",
            plan.kept
        );
    }

    #[test]
    fn test_kept_when_still_on_disk() {
        // A local directory the walk didn't list (a symlink, say) keeps
        // its cloud copy.
        let (local, entries) = local_tree("disk", &["a.txt"], &[]);
        std::fs::create_dir_all(local.join("enlazada")).unwrap();
        let folders = vec![folder(10, "ALBUM", 1), folder(11, "enlazada", 10)];
        let files = vec![file(1, 10, "a.txt"), file(2, 11, "z.jpg")];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert!(plan.deletions.is_empty(), "{:?}", plan.deletions);
        // An entry outside the directory is an error, not a deletion.
        let outside = vec![(PathBuf::from("/elsewhere/x"), false)];
        assert!(super::plan(&local, &entries, &outside, 10, &folders, &files).is_err());
    }

    #[test]
    fn test_count_nested_and_empty_folders() {
        // Empty folders count toward the limit too.
        let (local, entries) = local_tree("count", &["a.txt"], &[]);
        let folders = vec![
            folder(10, "ALBUM", 1),
            folder(11, "old", 10),
            folder(12, "e1", 11),
            folder(13, "e2", 11),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &[file(1, 10, "a.txt")]).unwrap();
        assert_eq!(paths(&plan), vec![("old", 0, 3)]);
        assert_eq!(plan.items(), 3);
    }

    #[test]
    fn test_on_disk_under_an_emoji_directory() {
        // `viaje😀/enlazada` is on disk but not walked (a symlink, say);
        // in the cloud it's `viaje/enlazada/`: kept, checked under the
        // real `viaje😀`.
        let (local, entries) = local_tree("emojidir", &["viaje😀/a.txt"], &["viaje😀"]);
        std::fs::create_dir_all(local.join("viaje😀/enlazada")).unwrap();
        let folders = vec![
            folder(10, "ALBUM", 1),
            folder(11, "viaje", 10),
            folder(12, "enlazada", 11),
        ];
        let files = vec![file(1, 11, "a.txt"), file(2, 12, "z.jpg")];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert!(plan.deletions.is_empty(), "{:?}", plan.deletions);
    }

    #[test]
    fn test_a_skeleton_with_no_files_keeps_the_cloud_one() {
        // `2019/` holds only an empty `jan/`: no file below it, so the
        // cloud's `2019` stays whole.
        let (local, entries) = local_tree("skeleton", &["a.txt"], &["2019", "2019/jan"]);
        let folders = vec![
            folder(10, "ALBUM", 1),
            folder(11, "2019", 10),
            folder(12, "jan", 11),
        ];
        let files = vec![
            file(1, 10, "a.txt"),
            file(2, 11, "x.jpg"),
            file(3, 11, "y.jpg"),
            file(4, 12, "z.jpg"),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert!(plan.deletions.is_empty(), "{:?}", plan.deletions);
        assert!(plan.kept[0].starts_with("2019/"), "{:?}", plan.kept);
    }

    #[test]
    fn test_a_folder_holding_protected_names_stays() {
        // `old/` is gone locally but holds `.git/` and a replacement's
        // copy: kept whole and said; `older/` goes.
        let (local, entries) = local_tree("protected", &["a.txt"], &[]);
        let folders = vec![
            folder(10, "ALBUM", 1),
            folder(11, "old", 10),
            folder(12, ".git", 11),
            folder(13, "older", 10),
        ];
        let files = vec![
            file(1, 10, "a.txt"),
            file(2, 12, "config"),
            file(3, 11, "v.o2-replacing-7.mp4"),
            file(4, 13, "w.txt"),
        ];
        let plan = plan(&local, &entries, &[], 10, &folders, &files).unwrap();
        assert_eq!(paths(&plan), vec![("older", 1, 1)]);
        assert!(
            plan.kept.iter().any(|k| k.starts_with("old/: kept")),
            "{:?}",
            plan.kept
        );
    }

    #[test]
    fn test_clashing_folder_names_stop_the_plan() {
        let (local, entries) = local_tree(
            "clashdir",
            &["viaje😀/a.txt", "viaje/b.txt"],
            &["viaje😀", "viaje"],
        );
        let folders = vec![folder(10, "ALBUM", 1), folder(11, "viaje", 10)];
        let err = plan(&local, &entries, &[], 10, &folders, &[]).unwrap_err();
        assert!(err.contains("viaje and viaje😀"), "{err}");
    }
}
