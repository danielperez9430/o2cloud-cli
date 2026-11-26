//! `labels`: the account's albums and what O2 Cloud groups by itself —
//! moments, places, and people once face recognition is turned on in its
//! website — and their photos and videos.

use std::collections::HashMap;

use super::{format_date, format_date_iso, print_json, Tree};
use crate::api::{Label, MediaDetails, MediaItem, O2Client};
use crate::config::AuthConfig;
use crate::error::O2Error;
use crate::util::format_size;

/// Which labels to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum LabelKind {
    /// All of them.
    All,
    /// What O2 Cloud gathers by date: trips, days, "a year ago"…
    Moments,
    /// Photos grouped by where they were taken.
    Places,
    /// Faces O2 Cloud has recognised (once turned on in its website).
    People,
    /// Albums made by people, the account's own or shared with it.
    Albums,
}

impl LabelKind {
    fn origins(self) -> &'static [&'static str] {
        match self {
            LabelKind::All => &[
                "flashback",
                "event",
                "geolocalized",
                "face",
                "omh",
                "shared_label",
            ],
            LabelKind::Moments => &["flashback", "event"],
            LabelKind::Places => &["geolocalized"],
            LabelKind::People => &["face"],
            LabelKind::Albums => &["omh", "shared_label"],
        }
    }
}

/// What a label is, from its origin.
fn kind_of(label: &Label) -> &'static str {
    match label.origin.as_deref() {
        Some("flashback") | Some("event") => "moment",
        Some("geolocalized") => "place",
        Some("face") => "person",
        Some("omh") | Some("shared_label") => "album",
        _ => "other",
    }
}

/// `{"lat", "lon"}`, or null.
pub(crate) fn location_json(location: Option<(f64, f64)>) -> serde_json::Value {
    match location {
        Some((lat, lon)) => serde_json::json!({ "lat": lat, "lon": lon }),
        None => serde_json::Value::Null,
    }
}

/// A photo or video's own fields for `--json`: its kind, where it was
/// taken and its thumbnails (signed URLs that open without the session,
/// for a while: a small one for lists, a large one to look at it).
pub(crate) fn details_json(details: &MediaDetails) -> serde_json::Value {
    let (thumbnail, preview) = details.thumbnail_and_preview();
    serde_json::json!({
        "type": details.mediatype,
        "location": location_json(details.location()),
        "thumbnail": thumbnail.map(|t| t.absolute_url()),
        "preview": preview.map(|t| t.absolute_url()),
    })
}

/// A label for `--json`. `cover` is what O2 knows of its cover photo: the
/// label's location and thumbnail come from it.
fn label_json(label: &Label, cover: Option<&MediaDetails>) -> serde_json::Value {
    let location = cover.and_then(MediaDetails::location);
    let thumbnail = cover
        .and_then(|c| c.thumbnail_and_preview().0)
        .map(|t| t.absolute_url());
    serde_json::json!({
        "id": label.id,
        "name": label.name,
        "kind": kind_of(label),
        "origin": label.origin,
        "count": label.items.len(),
        "date": format_date_iso(label.date),
        "cover": label.coverid,
        "location": location_json(location),
        "thumbnail": thumbnail,
        "items": label.items,
    })
}

/// What O2 knows of each label's cover (or its first photo, if it has no
/// cover): its location and thumbnails. One request for every 1,000.
async fn covers(
    client: &O2Client,
    auth: &AuthConfig,
    labels: &[Label],
) -> Result<HashMap<u64, MediaDetails>, O2Error> {
    let mut photos: Vec<u64> = labels
        .iter()
        .flat_map(|l| l.coverid.into_iter().chain(l.items.first().copied()))
        .collect();
    photos.sort_unstable();
    photos.dedup();
    let mut details_of_photo = HashMap::new();
    for chunk in photos.chunks(1000) {
        for (media, details) in client.get_media_details_by_ids(chunk, auth).await? {
            details_of_photo.insert(media.id, details);
        }
    }
    Ok(labels
        .iter()
        .filter_map(|l| {
            let cover = l.coverid.and_then(|id| details_of_photo.get(&id));
            let first = l.items.first().and_then(|id| details_of_photo.get(id));
            Some((l.id, cover.or(first)?.clone()))
        })
        .collect())
}

/// List the labels of `kind`, newest first.
pub async fn list_labels(
    client: &O2Client,
    auth: &AuthConfig,
    kind: LabelKind,
    json: bool,
) -> Result<(), O2Error> {
    let mut labels = client.get_labels(kind.origins(), auth).await?;
    labels.sort_by_key(|l| std::cmp::Reverse(l.date.unwrap_or(0)));
    if json {
        // Without covers the labels are still worth listing.
        let covers = match covers(client, auth, &labels).await {
            Ok(covers) => covers,
            Err(e) => {
                crate::log::warn(
                    "labels",
                    "covers not fetched",
                    serde_json::json!({ "error": e.to_string() }),
                );
                HashMap::new()
            }
        };
        print_json(&serde_json::Value::Array(
            labels
                .iter()
                .map(|l| label_json(l, covers.get(&l.id)))
                .collect(),
        ));
        return Ok(());
    }
    if labels.is_empty() {
        if kind == LabelKind::People {
            eprintln!(
                "No people. O2 Cloud groups faces only once face recognition is turned on \
                 in its website (cloud.o2online.es), and it takes a while after that."
            );
        } else {
            eprintln!("No labels.");
        }
        return Ok(());
    }
    println!(
        "{:<12}  {:<7}  {:>6}  {:<16}  NAME",
        "ID", "KIND", "PHOTOS", "DATE"
    );
    for label in &labels {
        println!(
            "{:<12}  {:<7}  {:>6}  {:<16}  {}",
            label.id,
            kind_of(label),
            label.items.len(),
            format_date(label.date),
            label.name.as_deref().unwrap_or("-")
        );
    }
    eprintln!(
        "\n  {} label(s). `o2cloud labels <id or name>` lists one's photos; \
         `o2cloud download-dir --label <id or name> -o <dir>` downloads them.",
        labels.len()
    );
    Ok(())
}

/// The label `target` names: its ID, or its name (any case) if only one
/// has it.
pub async fn find_label(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
) -> Result<Label, O2Error> {
    let labels = client.get_labels(LabelKind::All.origins(), auth).await?;
    if let Ok(id) = target.parse::<u64>() {
        if let Some(label) = labels.iter().find(|l| l.id == id) {
            return Ok(label.clone());
        }
    }
    let wanted = target.to_lowercase();
    let matches: Vec<&Label> = labels
        .iter()
        .filter(|l| l.name.as_deref().map(str::to_lowercase).as_deref() == Some(wanted.as_str()))
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(O2Error::NotFound(format!(
            "no label '{}'. `o2cloud labels` lists them",
            target
        ))),
        many => Err(O2Error::Ambiguous(format!(
            "{} labels are named '{}': {}. Use the ID",
            many.len(),
            target,
            many.iter()
                .map(|l| format!("{} ({})", l.id, kind_of(l)))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// A label's photos and videos, asked for by ID (1,000 at a time).
pub async fn label_media(
    client: &O2Client,
    auth: &AuthConfig,
    label: &Label,
) -> Result<Vec<MediaItem>, O2Error> {
    let mut media = Vec::with_capacity(label.items.len());
    for chunk in label.items.chunks(1000) {
        media.extend(client.get_media_by_ids(chunk, auth).await?);
    }
    Ok(media)
}

/// List the photos and videos of the label `target`.
pub async fn show_label(
    client: &O2Client,
    auth: &AuthConfig,
    target: &str,
    json: bool,
) -> Result<(), O2Error> {
    let label = find_label(client, auth, target).await?;
    let mut media = Vec::with_capacity(label.items.len());
    for chunk in label.items.chunks(1000) {
        media.extend(client.get_media_details_by_ids(chunk, auth).await?);
    }
    media.sort_by_key(|(m, _)| m.creationdate.or(m.date).unwrap_or(0));
    let paths = Paths::new(client, auth).await?;
    if json {
        let files: Vec<serde_json::Value> = media
            .iter()
            .map(|(m, details)| {
                let mut file = serde_json::json!({
                    "id": m.id,
                    "name": m.name,
                    "size": m.size,
                    "date": format_date_iso(m.creationdate.or(m.date)),
                    "path": paths.of(m),
                });
                merge(&mut file, details_json(details));
                file
            })
            .collect();
        // Its cover, or its first photo as O2 lists them (as `labels`).
        let by_id = |id: Option<u64>| media.iter().find(|(m, _)| Some(m.id) == id);
        let cover = by_id(label.coverid)
            .or_else(|| by_id(label.items.first().copied()))
            .map(|(_, d)| d);
        let mut out = label_json(&label, cover);
        out["files"] = files.into();
        print_json(&out);
        return Ok(());
    }
    let media: Vec<MediaItem> = media.into_iter().map(|(m, _)| m).collect();
    eprintln!(
        "{} ({}, {})",
        label.name.as_deref().unwrap_or("-"),
        kind_of(&label),
        format_date(label.date)
    );
    let mut total = 0;
    for m in &media {
        total += m.size.unwrap_or(0);
        println!(
            "  {:<12} {:>10}  {}  {}",
            m.id,
            m.size.map(format_size).unwrap_or_default(),
            format_date(m.creationdate),
            paths
                .of(m)
                .unwrap_or_else(|| m.name.clone().unwrap_or_default())
        );
    }
    eprintln!(
        "\n  {} file(s), {} in total",
        media.len(),
        format_size(total)
    );
    Ok(())
}

/// `extra`'s fields added to the object `into`.
pub(crate) fn merge(into: &mut serde_json::Value, extra: serde_json::Value) {
    if let (Some(into), serde_json::Value::Object(extra)) = (into.as_object_mut(), extra) {
        into.extend(extra);
    }
}

/// Files' paths in O2. Asked for by ID or by kind, O2 leaves out their
/// folder: it comes from the account's listing (cached between runs).
pub(crate) struct Paths {
    /// Each listed file's folder; `None` for the root (O2 sends no folder
    /// for files there).
    folder_of: HashMap<u64, Option<u64>>,
    /// Every folder's path.
    dirs: HashMap<u64, String>,
}

impl Paths {
    pub(crate) async fn new(client: &O2Client, auth: &AuthConfig) -> Result<Self, O2Error> {
        let folders = client.get_all_folders(auth).await?;
        let media = client.get_all_media(auth).await?;
        let folder_name: HashMap<u64, String> =
            folders.iter().map(|f| (f.id, f.name.clone())).collect();
        let tree = Tree::new(&folders, &folder_name, &[]);
        let dirs = folders.iter().map(|f| (f.id, tree.path(f.id))).collect();
        let folder_of = media.iter().map(|m| (m.id, m.folderid)).collect();
        Ok(Paths { folder_of, dirs })
    }

    /// `m`'s path: in its folder (`None` if that folder isn't known), at
    /// the root if it has none, or `None` if it isn't in the listing yet
    /// (uploaded elsewhere moments ago).
    pub(crate) fn of(&self, m: &MediaItem) -> Option<String> {
        let folder = match m.folderid {
            Some(folder) => Some(folder),
            None => *self.folder_of.get(&m.id)?,
        };
        let dir = match folder {
            Some(folder) => self.dirs.get(&folder)?.as_str(),
            None => "/",
        };
        let name = m.name.as_deref().unwrap_or("?");
        Some(if dir == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", dir.trim_end_matches('/'), name)
        })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_paths() {
        let item = |id: u64, folder: Option<u64>| -> MediaItem {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": format!("f{id}.jpg"), "folder": folder
            }))
            .unwrap()
        };
        let paths = Paths {
            folder_of: HashMap::from([(1, None), (2, Some(10)), (3, Some(11))]),
            dirs: HashMap::from([(10, "/Fotos".to_string()), (11, "/Vacía".to_string())]),
        };
        // No folder: the root. Its own folder, or the listing's.
        assert_eq!(paths.of(&item(1, None)).as_deref(), Some("/f1.jpg"));
        assert_eq!(paths.of(&item(2, None)).as_deref(), Some("/Fotos/f2.jpg"));
        // A folder with no other file in the listing.
        assert_eq!(paths.of(&item(9, Some(11))).as_deref(), Some("/Vacía/f9.jpg"));
        // A folder that isn't known, or a file not listed yet: no path.
        assert_eq!(paths.of(&item(8, Some(99))), None);
        assert_eq!(paths.of(&item(7, None)), None);
    }

    use super::*;

    fn label(origin: &str) -> Label {
        serde_json::from_value(serde_json::json!({
            "labelid": 1, "name": "x", "origin": origin, "items": [1, 2], "date": 0
        }))
        .unwrap()
    }

    #[test]
    fn test_kinds() {
        assert_eq!(kind_of(&label("flashback")), "moment");
        assert_eq!(kind_of(&label("event")), "moment");
        assert_eq!(kind_of(&label("geolocalized")), "place");
        assert_eq!(kind_of(&label("face")), "person");
        assert_eq!(kind_of(&label("omh")), "album");
        assert_eq!(kind_of(&label("tagged")), "other");
        // Every kind asks for its own origins, and All for all of them.
        for kind in [
            LabelKind::Moments,
            LabelKind::Places,
            LabelKind::People,
            LabelKind::Albums,
        ] {
            for origin in kind.origins() {
                assert!(LabelKind::All.origins().contains(origin));
            }
        }
        let json = label_json(&label("flashback"), None);
        assert!(json["location"].is_null() && json["thumbnail"].is_null());
        assert_eq!(
            (json["kind"].as_str(), json["count"].as_u64()),
            (Some("moment"), Some(2))
        );
    }
}
