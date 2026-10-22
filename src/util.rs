//! Small formatting helpers shared between `api.rs` and `ops.rs`.

/// Format a byte count as a human-readable size (e.g. `1.5 KB`).
pub fn format_size(bytes: u64) -> String {
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

/// Format a `SystemTime` as the compact ISO 8601 timestamp the SAPI
/// expects: `YYYYMMDDTHHMMSS`.
pub fn system_time_to_compact_iso(t: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
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

/// Guess a MIME content-type from a file's extension.
pub fn mime_type_from_name(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "txt" | "md" | "rs" | "toml" | "lock" | "py" | "js" | "ts" | "html" | "css" | "json"
        | "xml" | "yaml" | "yml" | "csv" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" | "bz2" | "xz" | "7z" => "application/octet-stream",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "php" => "application/x-httpd-php",
        _ => "application/octet-stream",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(18), "18 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
        assert_eq!(format_size(1099511627776), "1.0 TB");
    }

    #[test]
    fn test_mime_type_from_name() {
        assert_eq!(mime_type_from_name("file.txt"), "text/plain");
        assert_eq!(mime_type_from_name("photo.png"), "image/png");
        assert_eq!(mime_type_from_name("doc.pdf"), "application/pdf");
        assert_eq!(mime_type_from_name("movie.mp4"), "video/mp4");
        assert_eq!(mime_type_from_name("song.mp3"), "audio/mpeg");
        assert_eq!(mime_type_from_name("clip.MOV"), "video/quicktime");
        assert_eq!(mime_type_from_name("song.flac"), "audio/flac");
        assert_eq!(mime_type_from_name("backup.zip"), "application/zip");
        assert_eq!(mime_type_from_name("script.php"), "application/x-httpd-php");
        assert_eq!(
            mime_type_from_name("unknown.xyz"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_system_time_to_compact_iso() {
        use std::time::UNIX_EPOCH;
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1705314645);
        assert_eq!(system_time_to_compact_iso(t), "20240115T103045");
    }
}
