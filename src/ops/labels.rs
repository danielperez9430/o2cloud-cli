//! `labels`: the account's albums and what O2 Cloud groups by itself —
//! moments, places, and people once face recognition is turned on in its
//! website — and their photos and videos.

use std::collections::HashMap;

use super::{format_date, format_date_iso, print_json, resolve_path};
use crate::api::{Label, MediaItem, O2Client};
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

fn label_json(label: &Label) -> serde_json::Value {
    serde_json::json!({
        "id": label.id,
        "name": label.name,
        "kind": kind_of(label),
        "origin": label.origin,
        "count": label.items.len(),
        "date": format_date_iso(label.date),
        "cover": label.coverid,
        "items": label.items,
    })
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
        print_json(&serde_json::Value::Array(
            labels.iter().map(label_json).collect(),
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
    let mut media = label_media(client, auth, &label).await?;
    media.sort_by_key(|m| m.creationdate.unwrap_or(0));
    let folders = client.get_all_folders(auth).await?;
    let folder_name: HashMap<u64, String> =
        folders.iter().map(|f| (f.id, f.name.clone())).collect();
    let path_of = |m: &MediaItem| -> Option<String> {
        let dir = m
            .folderid
            .map(|f| resolve_path(f, &folder_name, &folders))?;
        let name = m.name.as_deref().unwrap_or("?");
        Some(if dir == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", dir, name)
        })
    };
    if json {
        let files: Vec<serde_json::Value> = media
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "name": m.name,
                    "size": m.size,
                    "date": format_date_iso(m.creationdate),
                    "path": path_of(m),
                })
            })
            .collect();
        let mut out = label_json(&label);
        out["files"] = files.into();
        print_json(&out);
        return Ok(());
    }
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
            path_of(m).unwrap_or_else(|| m.name.clone().unwrap_or_default())
        );
    }
    eprintln!(
        "\n  {} file(s), {} in total",
        media.len(),
        format_size(total)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
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
        let json = label_json(&label("flashback"));
        assert_eq!(
            (json["kind"].as_str(), json["count"].as_u64()),
            (Some("moment"), Some(2))
        );
    }
}
