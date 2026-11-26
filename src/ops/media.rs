//! `media`: the account's files of one kind, wherever they are: what O2
//! Cloud's website shows as Música, Documentos and its gallery.

use super::labels::{details_json, merge, Paths};
use super::{format_date, format_date_iso, print_json};
use crate::api::O2Client;
use crate::config::AuthConfig;
use crate::error::O2Error;
use crate::util::format_size;

/// Which kind of files to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum MediaKind {
    /// Photos, newest first.
    Photos,
    /// Videos, newest first.
    Videos,
    /// Music and other audio, by name.
    Music,
    /// Everything else O2 keeps (PDFs, documents, archives…), by name.
    Documents,
}

impl MediaKind {
    /// O2's name for it in `/sapi/media/<kind>`.
    fn sapi(self) -> &'static str {
        match self {
            MediaKind::Photos => "picture",
            MediaKind::Videos => "video",
            MediaKind::Music => "audio",
            MediaKind::Documents => "file",
        }
    }

    /// Whether a file named `name` is of this kind, by its extension (for
    /// files O2 hasn't classified yet).
    fn matches(self, name: &str) -> bool {
        let mime = super::mime_type_from_name(name);
        match self {
            MediaKind::Photos => mime.starts_with("image/"),
            MediaKind::Videos => mime.starts_with("video/"),
            MediaKind::Music => mime.starts_with("audio/"),
            MediaKind::Documents => !["image/", "video/", "audio/"]
                .iter()
                .any(|p| mime.starts_with(p)),
        }
    }

    fn newest_first(self) -> bool {
        matches!(self, MediaKind::Photos | MediaKind::Videos)
    }
}

/// List the account's files of `kind`.
pub async fn list_media_kind(
    client: &O2Client,
    auth: &AuthConfig,
    kind: MediaKind,
    json: bool,
) -> Result<(), O2Error> {
    let mut media = client.get_media_of_kind(kind.sapi(), auth).await?;
    let paths = Paths::new(client, auth).await?;
    // O2's list by kind shows a new file 5–15 s after it's uploaded; what
    // this CLI uploaded moments ago is added from its own note of it.
    let listed: std::collections::HashSet<u64> = media.iter().map(|(m, _)| m.id).collect();
    let fresh: Vec<u64> = crate::cache::provisional_ids(&auth.msisdn)
        .into_iter()
        .filter(|id| !listed.contains(id))
        .collect();
    if !fresh.is_empty() {
        for item in client.get_all_media(auth).await? {
            let name = item.name.as_deref().unwrap_or("");
            if fresh.contains(&item.id) && kind.matches(name) {
                media.push((item, crate::api::MediaDetails::default()));
            }
        }
    }
    if kind.newest_first() {
        media.sort_by_key(|(m, _)| std::cmp::Reverse(m.creationdate.or(m.date).unwrap_or(0)));
    } else {
        media.sort_by_cached_key(|(m, _)| m.name.as_deref().unwrap_or("").to_lowercase());
    }
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
                    "duration": details.duration(),
                    // Signed, like the thumbnails: it opens (and streams,
                    // with ranges) without the session, for a while.
                    "url": m.url.as_deref().map(crate::api::absolute_url),
                });
                merge(&mut file, details_json(details));
                file
            })
            .collect();
        print_json(&serde_json::Value::Array(files));
        return Ok(());
    }
    if media.is_empty() {
        eprintln!("No files of this kind.");
        return Ok(());
    }
    let mut total = 0;
    for (m, _) in &media {
        total += m.size.unwrap_or(0);
        println!(
            "  {:<12} {:>10}  {}  {}",
            m.id,
            m.size.map(format_size).unwrap_or_default(),
            format_date(m.creationdate.or(m.date)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_by_name() {
        assert!(MediaKind::Music.matches("song.mp3"));
        assert!(MediaKind::Documents.matches("report.pdf"));
        assert!(!MediaKind::Documents.matches("photo.jpg"));
        assert!(MediaKind::Photos.matches("photo.JPG"));
        assert!(MediaKind::Videos.matches("clip.mp4"));
    }
}
