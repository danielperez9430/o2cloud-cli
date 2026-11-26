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
    if kind.newest_first() {
        media.sort_by_key(|(m, _)| std::cmp::Reverse(m.creationdate.unwrap_or(0)));
    } else {
        media.sort_by_cached_key(|(m, _)| m.name.as_deref().unwrap_or("").to_lowercase());
    }
    let paths = Paths::new(client, auth).await?;
    if json {
        let files: Vec<serde_json::Value> = media
            .iter()
            .map(|(m, details)| {
                let mut file = serde_json::json!({
                    "id": m.id,
                    "name": m.name,
                    "size": m.size,
                    "date": format_date_iso(m.creationdate),
                    "path": paths.of(m),
                    "duration": details.duration(),
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
