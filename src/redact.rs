//! Keeps secrets out of everything the CLI writes for people to read or
//! send: error messages, traces, the run log and `debug-report`.
//!
//! Two layers: the session's own values (registered once they're known,
//! so they're caught wherever they turn up), and patterns for what has
//! the shape of a secret (`validationkey=…`, `JSESSIONID=…`, `Bearer …`,
//! and URLs, whose query and, for downloads, path carry tokens).

use std::sync::Mutex;

/// The secret values seen so far, with the name that replaces them.
static SECRETS: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

/// Treat `value` as a secret from now on: it's replaced by `<label>`.
/// Values too short to be secrets (or empty) are ignored, so they can't
/// blank out ordinary text.
pub fn register(value: &str, label: &'static str) {
    let value = value.trim();
    if value.len() < 6 {
        return;
    }
    let mut secrets = SECRETS.lock().unwrap_or_else(|e| e.into_inner());
    if !secrets.iter().any(|(v, _)| v == value) {
        secrets.push((value.to_string(), label));
        // Longest first, so a value that contains another goes whole.
        secrets.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
    }
}

/// Register every secret of a session.
pub fn register_auth(auth: &crate::config::AuthConfig) {
    register(&auth.jsessionid, "jsessionid");
    register(&auth.validationkey, "validationkey");
    register(&auth.access_token, "token");
    register(&auth.encryption_token, "token");
    register(&auth.msisdn, "phone");
    register(&auth.device_id, "device-id");
    if let Some(client) = &auth.oauth_client {
        register(&client.id, "oauth-client");
        register(&client.secret, "oauth-secret");
    }
}

/// Keys whose value is always a secret, as `key=value` or `key: value`.
const SECRET_KEYS: [&str; 11] = [
    "validationkey",
    "jsessionid",
    "access_token",
    "refresh_token",
    "id_token",
    "encryption_token",
    "encryption-token",
    "client_secret",
    "code_verifier",
    "authorization",
    "\"code",
];

/// `text` with every secret replaced: registered values, URLs (see
/// `url`), the values of `SECRET_KEYS`, and bearer tokens.
pub fn text(text: &str) -> String {
    let mut out = text.to_string();
    {
        let secrets = SECRETS.lock().unwrap_or_else(|e| e.into_inner());
        for (value, label) in secrets.iter() {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("<{}>", label));
            }
        }
    }
    out = urls(&out);
    // Bearer tokens first: `Authorization: Bearer <token>` has two words.
    out = bearer(&out);
    for key in SECRET_KEYS {
        out = key_values(&out, key);
    }
    out
}

/// A URL as it may be shown: scheme and host, the path only for SAPI
/// endpoints (a download link carries a token in its path), and of the
/// query only `action`.
pub fn url(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or("?");
    let path = if url.path().starts_with("/sapi/") {
        url.path().to_string()
    } else if url.path().len() > 1 {
        "/…".to_string()
    } else {
        String::new()
    };
    let action = url
        .query_pairs()
        .find(|(k, _)| k == "action")
        .map(|(_, v)| format!("?action={}", v))
        .unwrap_or_default();
    format!("{}://{}{}{}", url.scheme(), host, path, action)
}

/// Every `http://` or `https://` URL in `text` through `url`.
fn urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let start = match (rest.find("https://"), rest.find("http://")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '<' | '>' | ','))
            .unwrap_or(tail.len());
        match url::Url::parse(&tail[..end]) {
            Ok(parsed) => out.push_str(&url(&parsed)),
            Err(_) => out.push_str("<url>"),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// `key=value` and `key: value` (any case) with the value replaced.
fn key_values(text: &str, key: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while let Some(found) = lower[pos..].find(key) {
        let key_end = pos + found + key.len();
        let after = &text[key_end..];
        let sep = if after.starts_with('=') {
            1
        } else if after.starts_with(": ") {
            2
        } else if after.starts_with("\":\"") {
            3
        } else if after.starts_with("\": \"") {
            4
        } else {
            out.push_str(&text[pos..key_end]);
            pos = key_end;
            continue;
        };
        let value_start = key_end + sep;
        let value_len = text[value_start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '&' | ';' | ',' | '"' | ')'))
            .unwrap_or(text.len() - value_start);
        out.push_str(&text[pos..value_start]);
        out.push_str(if value_len > 0 { "<redacted>" } else { "" });
        pos = value_start + value_len;
    }
    out.push_str(&text[pos..]);
    out
}

/// `Bearer <token>` with the token replaced.
fn bearer(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("Bearer ") {
        out.push_str(&rest[..i + 7]);
        let tail = &rest[i + 7..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '"')
            .unwrap_or(tail.len());
        out.push_str("<redacted>");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Command-line arguments with the values of options that take secrets
/// (`--jsessionid`, `--validationkey`, `--oauth-client-secret`…) masked,
/// as `--option value` or `--option=value`, and each through `text`.
pub fn args(args: &[String]) -> Vec<String> {
    let secret = |flag: &str| {
        let flag = flag.trim_start_matches('-').to_ascii_lowercase();
        [
            "jsessionid",
            "validationkey",
            "secret",
            "token",
            "client-id",
            "password",
            "phone",
        ]
        .iter()
        .any(|s| flag.contains(s))
    };
    let mut out = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for arg in args {
        if mask_next {
            out.push("<redacted>".to_string());
            mask_next = false;
            continue;
        }
        match arg.split_once('=') {
            Some((flag, _)) if flag.starts_with("--") && secret(flag) => {
                out.push(format!("{}=<redacted>", flag));
            }
            _ if arg.starts_with('-') && secret(arg) => {
                out.push(arg.clone());
                mask_next = true;
            }
            _ => out.push(text(arg)),
        }
    }
    out
}

/// A stand-in for a file or folder name in a report: a short hash of it
/// (with `salt`, so reports can't be matched against each other), its
/// extension, its length in bytes, and whether it has what breaks names:
/// non-ASCII letters, a trailing dot or space, or a Windows reserved name.
/// Each `/` or `\` separated part of a path gets its own.
pub fn pseudonym(path: &str, salt: &str) -> String {
    use sha2::{Digest, Sha256};
    let part = |name: &str| -> String {
        if name.is_empty() || name == "." || name == ".." || name == "~" {
            return name.to_string();
        }
        let hash = Sha256::digest(format!("{}{}", salt, name).as_bytes());
        let hex: String = hash[..3].iter().map(|b| format!("{:02x}", b)).collect();
        // Only what looks like a real extension: `2019.Boda Ana y Luis`
        // has none.
        let ext = std::path::Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|e| format!(".{}", e))
            .unwrap_or_default();
        let mut flags = String::new();
        if !name.is_ascii() {
            flags.push_str(",utf8");
        }
        if name.ends_with('.') || name.ends_with(' ') {
            flags.push_str(",trailing");
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            flags.push_str(",reserved");
        }
        format!("n{}{}[{}b{}]", hex, ext, name.len(), flags)
    };
    let mut out = String::new();
    let mut rest = path;
    while let Some(i) = rest.find(['/', '\\']) {
        out.push_str(&part(&rest[..i]));
        out.push_str(&rest[i..i + 1]);
        rest = &rest[i + 1..];
    }
    out.push_str(&part(rest));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patterns() {
        let t = text(
            "GET https://cloud.o2online.es/sapi/media?action=get&validationkey=abcdef123456 \
             failed; validationkey=zzzzzz99; JSESSIONID: 0123456789ab and \
             Authorization: Bearer eyJhbGciOi.payload.sig done",
        );
        assert!(!t.contains("abcdef123456"), "{t}");
        assert!(!t.contains("zzzzzz99"), "{t}");
        assert!(!t.contains("0123456789ab"), "{t}");
        assert!(!t.contains("eyJhbGciOi"), "{t}");
        assert!(
            t.contains("https://cloud.o2online.es/sapi/media?action=get"),
            "{t}"
        );
        assert!(t.ends_with("done"), "{t}");
        let t = text(
            r#"{"access_token": "AT-123456789", "encryption-token=ET-987654321", "code":"OC-55555"}"#,
        );
        assert!(
            !t.contains("AT-1") && !t.contains("ET-9") && !t.contains("OC-5"),
            "{t}"
        );
        // A download link: its path is a token too.
        let t = text("from https://dl.o2online.es/a1b2c3d4e5/f/photo.jpg?sig=xyz).");
        assert_eq!(t, "from https://dl.o2online.es/…).");
        // Plain text is left alone.
        assert_eq!(text("3 files, 12 MB"), "3 files, 12 MB");
    }

    #[test]
    fn test_registered_values() {
        register("S3CR3T-SESSION-42", "jsessionid");
        register("abc", "token"); // too short to count
        let t = text("session S3CR3T-SESSION-42 in abc");
        assert_eq!(t, "session <jsessionid> in abc");
    }

    #[test]
    fn test_args() {
        let a: Vec<String> = [
            "login",
            "--jsessionid",
            "JS-123456789",
            "--validationkey=VK-987654321",
            "--oauth-client-secret",
            "OCS-55555555",
            "--phone",
            "600111222",
            "upload-dir",
            "./ALBUM",
            "-j",
            "4",
        ]
        .map(String::from)
        .to_vec();
        let r = args(&a).join(" ");
        assert!(
            !r.contains("JS-123456789") && !r.contains("VK-987654321"),
            "{r}"
        );
        assert!(
            !r.contains("OCS-55555555") && !r.contains("600111222"),
            "{r}"
        );
        assert!(r.ends_with("upload-dir ./ALBUM -j 4"), "{r}");
    }

    #[test]
    fn test_pseudonym() {
        let p = pseudonym("ALBUM/Navidad 2019/foto Ñandú.JPG", "salt");
        assert!(!p.contains("Navidad") && !p.contains("Ñandú"), "{p}");
        assert_eq!(p.matches('/').count(), 2);
        assert!(p.ends_with(".JPG[16b,utf8]"), "{p}");
        assert_eq!(pseudonym("a/b", "s"), pseudonym("a/b", "s"));
        assert_ne!(pseudonym("a/b", "s"), pseudonym("a/b", "t"));
        assert!(pseudonym("CON.txt", "s").contains("reserved"));
        assert!(pseudonym("x.", "s").contains("trailing"));
        // A dot in a folder's name isn't an extension to keep.
        assert!(!pseudonym("2019.Boda Ana y Luis", "s").contains("Boda"));
    }
}
