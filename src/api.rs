use std::time::Duration;

use reqwest::{header, Client};
use serde::{Deserialize, Serialize};

use crate::config::AuthConfig;
use crate::error::O2Error;

/// Base URL for the O2 Cloud SAPI (Synchronoss API).
const BASE_URL: &str = "https://cloud.o2online.es";
/// Telefónica's OAuth authorization endpoint, where the login redirects.
const AUTHORIZE_URL_PREFIX: &str = "https://apiseg.telefonica.es/";
/// Separate host for binary uploads.
const UPLOAD_URL: &str = "https://upload.cloud.o2online.es";

/// Telefónica OpenID Connect endpoints used by the optional token-login
/// flow (the one the mobile app uses). This flow exchanges the code on
/// the client side with an OAuth `client_id`/`client_secret`, which the
/// user must supply; see `login_native`.
const OIDC_AUTHORIZE_URL: &str =
    "https://apiseg.telefonica.es/openid/connect/auth/oauth/v2/o2/cus/authorize";
const OIDC_TOKEN_URL: &str =
    "https://apiseg.telefonica.es/openid/connect/auth/oauth/v2/o2/cus/token";
/// Redirect URI registered for that OAuth client; the browser is sent
/// here with `?code=...&state=...` once the user logs in.
const OIDC_REDIRECT_URI: &str = "https://cloud.o2online.es/ui/html/clientoauth.html";
/// User-Agent of the mobile client. SAPI serves the JSON API to this UA;
/// a browser UA gets the HTML page instead. Used for the token login and
/// for SAPI calls that carry the `oauth` token.
const APP_USER_AGENT: &str = "omh ios client 4.0.1";

/// `Origin` sent by the web client. Applied per request to the web-style
/// calls only (session bootstrap, the web login exchange and SAPI calls
/// on a web session). It is deliberately *not* a default header: the
/// native app sends no `Origin`, and a default can't be removed for a
/// single request, so leaving it default would leak `Origin` into the
/// native token login (`/token` at Telefónica and `/sapi/login/oauth`)
/// and the token-based SAPI calls, which breaks them.
const WEB_ORIGIN: &str = "https://cloud.o2online.es";

/// Per-request timeout applied to metadata/listing/login calls — never
/// to the raw upload/download of file bytes, which can legitimately take
/// much longer than this for large files.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Truncate `s` to at most `max` bytes without splitting a UTF-8 character.
fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// O2 Cloud's limits on names and nesting, measured live (2026-11-25):
/// a longer file name is `COM-1010` "Invalid filename", a longer folder
/// name `FOL-1000`, a deeper folder `FOL-1015`. A file name is limited in
/// UTF-8 bytes (129 `ñ`), a folder name in characters (255 `ñ` fit).
/// The root folder is level 0; folders can go 20 levels below it.
pub const MAX_FILE_NAME_BYTES: usize = 255;
pub const MAX_FOLDER_NAME_CHARS: usize = 255;
pub const MAX_FOLDER_LEVEL: usize = 20;

/// Why O2 Cloud won't take `name` as a file name, if it won't.
pub fn file_name_problem(name: &str) -> Option<String> {
    (name.len() > MAX_FILE_NAME_BYTES).then(|| {
        format!(
            "the name is {} bytes long; O2 Cloud takes file names of up to {} bytes \
             (UTF-8: fewer characters if they have accents or are from other alphabets)",
            name.len(),
            MAX_FILE_NAME_BYTES
        )
    })
}

/// Why O2 Cloud won't take `name` as a folder name, if it won't.
pub fn folder_name_problem(name: &str) -> Option<String> {
    let chars = name.chars().count();
    (chars > MAX_FOLDER_NAME_CHARS).then(|| {
        format!(
            "the name is {} characters long; O2 Cloud takes folder names of up to {}",
            chars, MAX_FOLDER_NAME_CHARS
        )
    })
}

/// What a folder or rename error means, for the codes that don't say.
fn limit_hint(err: &str) -> String {
    if err.contains("FOL-1015") {
        format!(
            " — O2 Cloud allows at most {} levels of folders below the root",
            MAX_FOLDER_LEVEL
        )
    } else if err.contains("COM-1010") {
        format!(
            " — the name may be too long: O2 Cloud takes file names of up to {} bytes",
            MAX_FILE_NAME_BYTES
        )
    } else {
        String::new()
    }
}

/// What an upload rejection means, for the codes that look like
/// something else. The server checks the content, not just the name:
/// tested with `.jpg` files, `MED-1050` is an empty file, one of zeros or
/// random bytes, or a HEIC named `.jpg`, and `MED-1000` a damaged JPEG.
fn rejection_hint(err: &str) -> &'static str {
    if err.contains("MED-1050") {
        " — the server checks the content: the file is empty or damaged, \
         or its format doesn't match its extension (e.g. a HEIC photo named .jpg); \
         retrying won't help"
    } else if err.contains("MED-1000") {
        " — the server couldn't read the file: it may be damaged or incomplete \
         (e.g. a leftover .tmp)"
    } else if err.contains("COM-1010") {
        " — the name may be too long: O2 Cloud takes file names of up to 255 bytes"
    } else {
        ""
    }
}

/// Extract the error from a SAPI error envelope, which the server can
/// send even with a 200 status:
/// `{"error":{"code":"PAPI-0000","message":"Unknown exception","parameters":[...]}}`.
/// Returns e.g. `Unknown exception (PAPI-0000)`, or `None` if `raw` is not
/// an error envelope.
fn sapi_error(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let error = value.get("error")?;
    let message = error
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error");
    let mut out = match error.get("code").and_then(|c| c.as_str()) {
        Some(code) => format!("{} ({})", message, code),
        None => message.to_string(),
    };
    let params: Vec<&str> = error
        .get("parameters")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|p| p["param"].as_str()).collect())
        .unwrap_or_default();
    if !params.is_empty() {
        out.push_str(&format!(" [params: {}]", params.join(", ")));
    }
    Some(out)
}

/// Retries for a call SAPI answers with 429 (Too Many Requests) or 503
/// (Service Unavailable), waiting 1 s, 2 s and 4 s (plus jitter) unless
/// the server sends `Retry-After`.
/// Files per page of the media listing (the most SAPI returns).
const MEDIA_PAGE_SIZE: usize = 1000;
/// A `from` later than any server time: `changes_since` then reports
/// nothing, only the server's current time.
const FAR_FUTURE_MS: u64 = 1 << 52;
/// How far back each change-log query reaches before the last one, so a
/// change logged a little late isn't missed (they are applied twice).
const CHANGES_OVERLAP_MS: u64 = 60_000;
/// How long a file the change log reported is asked for again while the
/// server doesn't return it, before the whole account is listed instead.
const PENDING_FOR_SECS: u64 = 600;

/// Pages of the media listing asked for at once after the first. Up to
/// `MEDIA_PAGES_AT_ONCE - 1` empty pages past the end are asked for too.
const MEDIA_PAGES_AT_ONCE: usize = 4;

const BUSY_RETRIES: u32 = 3;
/// Longest `Retry-After` honoured, so a bad header can't stall the CLI.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Retries for a call on a file SAPI is still validating, which it does
/// for a few seconds (~4–10 s) after an upload and reports as `MED-1017`.
const VALIDATION_RETRIES: u32 = 8;
const VALIDATION_RETRY_DELAY: Duration = Duration::from_millis(1500);

/// Whether `err` is SAPI's "media not validated yet" (`MED-1017`).
fn is_validating(err: &O2Error) -> bool {
    matches!(err, O2Error::Api(msg) if msg.contains("MED-1017"))
}

/// Run `call`, retrying while SAPI is still validating a just-uploaded
/// file (`MED-1017`), so `rm`, `mv` or `cp` right after an upload work.
async fn retry_while_validating<T, F, Fut>(mut call: F) -> Result<T, O2Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, O2Error>>,
{
    let mut attempt = 0;
    loop {
        match call().await {
            Err(e) if is_validating(&e) && attempt < VALIDATION_RETRIES => {
                attempt += 1;
                tokio::time::sleep(VALIDATION_RETRY_DELAY).await;
            }
            result => return result,
        }
    }
}

/// Whether a request creates something on the server (a folder, a
/// copy, a link…), which SAPI does with `action=save` or `action=copy`.
/// A 503 can come from a gateway after the server acted, so such a
/// request isn't retried on 503: it could make a duplicate.
fn creates_something(req: &reqwest::Request) -> bool {
    req.url()
        .query_pairs()
        .any(|(k, v)| k == "action" && (v == "save" || v == "copy"))
}

/// Whether `status` means "busy, try again": 429 (the request was
/// refused, so retrying is always safe), or 503 unless the request
/// `creates` something.
fn is_busy(status: reqwest::StatusCode, creates: bool) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (status == reqwest::StatusCode::SERVICE_UNAVAILABLE && !creates)
}

/// Wait before busy retry number `retry` (0-based): the server's
/// `Retry-After` in seconds if it sent one, else exponential backoff.
fn busy_wait(resp: &reqwest::Response, retry: u32) -> Duration {
    let retry_after = resp
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
    retry_after.unwrap_or_else(|| {
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() % 400)
            .unwrap_or(0);
        Duration::from_secs(1 << retry) + Duration::from_millis(100 + jitter as u64)
    })
}

/// The `upload/<kind>` route SAPI wants for a file's metadata changes:
/// photos, music and videos each have their own, and the generic `file`
/// one is refused for them. Checked against real uploads; extensions
/// not listed here are generic for SAPI (`update_file` falls back to
/// `file` anyway if a guess is wrong).
fn media_kind(name: &str) -> &'static str {
    let ext = match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_ascii_lowercase(),
        None => return "file",
    };
    match ext.as_str() {
        "mp3" | "m4a" | "wav" => "audio",
        "mp4" => "video",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" => "picture",
        _ => "file",
    }
}

/// `media` in one order whether it came from the cache or the server:
/// by name ignoring case (about how the server lists them), then by exact
/// name and ID. Output that keeps this order (`find`, files with the same
/// name in `ls`) is then the same either way.
fn in_listing_order(mut media: Vec<MediaItem>) -> Vec<MediaItem> {
    media.sort_by_cached_key(|m| {
        let name = m.name.clone().unwrap_or_default();
        (name.to_lowercase(), name, m.id)
    });
    media
}

/// What `changes_since` found.
#[derive(Debug, Default, PartialEq)]
struct Changes {
    /// Server time of the query: where the next one starts.
    requesttime: u64,
    /// Files added, changed, moved or renamed (`N` and `U`).
    changed: Vec<u64>,
    /// Files deleted (`D`).
    deleted: Vec<u64>,
    /// A kind of item it doesn't know, which might be a file.
    unknown: Option<String>,
}

/// Kinds of item in the change log that are files.
const FILE_KINDS: &[&str] = &["picture", "video", "audio", "file", "document"];
/// Kinds of item in the change log that aren't (folders are listed afresh
/// by every command; labels, tags and albums aren't used).
const OTHER_KINDS: &[&str] = &["folder", "label", "tag", "mediaset", "album", "playlist"];

/// Parse a `/sapi/profile/changes` response:
/// `{"requesttime":…,"data":{"picture":{"U":[ids],"D":[ids]},"folder":{…},
/// "changes":[{"type":"profile"},…]}}`.
fn parse_changes(raw: &str) -> Result<Changes, O2Error> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        O2Error::Parse(format!(
            "Failed to parse changes: {} — body: {}",
            e,
            truncate_utf8(raw, 300)
        ))
    })?;
    let requesttime = v["requesttime"].as_u64().ok_or_else(|| {
        O2Error::Parse(format!(
            "changes without requesttime: {}",
            truncate_utf8(raw, 300)
        ))
    })?;
    let mut changes = Changes {
        requesttime,
        ..Default::default()
    };
    let Some(data) = v.get("data").and_then(|d| d.as_object()) else {
        return Ok(changes);
    };
    for (kind, ops) in data {
        // `changes` lists account settings that changed, not items.
        let Some(ops) = ops.as_object() else { continue };
        if OTHER_KINDS.contains(&kind.as_str()) {
            continue;
        }
        if !FILE_KINDS.contains(&kind.as_str()) {
            changes.unknown = Some(kind.clone());
            continue;
        }
        for (op, ids) in ops {
            let ids = ids
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_u64().or_else(|| id.as_str()?.parse().ok()));
            match op.as_str() {
                "N" | "U" => changes.changed.extend(ids),
                "D" => changes.deleted.extend(ids),
                _ => changes.unknown = Some(format!("{kind}:{op}")),
            }
        }
    }
    Ok(changes)
}

/// The folder list from a `/sapi/media/folder?action=get` response.
fn parse_folders(status: reqwest::StatusCode, raw_body: &str) -> Result<Vec<FolderEntry>, O2Error> {
    if !status.is_success() {
        return Err(O2Error::Api(format!(
            "Get folders failed (status {}): {}",
            status,
            truncate_utf8(raw_body, 300)
        )));
    }

    #[derive(Deserialize)]
    struct FolderData {
        folders: Vec<FolderEntry>,
    }

    let body: RawResponse<FolderData> = serde_json::from_str(raw_body).map_err(|e| {
        O2Error::Parse(format!(
            "Failed to parse folder list: {} — body: {}",
            e,
            truncate_utf8(raw_body, 300)
        ))
    })?;
    Ok(body.data.folders)
}

/// Requests sent so far, counted for `O2CLOUD_TRACE`.
static REQUESTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether `O2CLOUD_TRACE` is set: each request is then logged to stderr
/// (method, host, path, SAPI `action`, status, time), and `main` prints
/// how many there were. Query strings, headers and bodies are left out:
/// they hold the session keys.
pub fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("O2CLOUD_TRACE").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// How many requests this run has sent.
pub fn requests_sent() -> usize {
    REQUESTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// `send`, logged when `tracing()`.
trait SendTraced {
    async fn send_traced(self) -> Result<reqwest::Response, reqwest::Error>;
}

impl SendTraced for reqwest::RequestBuilder {
    async fn send_traced(self) -> Result<reqwest::Response, reqwest::Error> {
        let n = REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if !tracing() {
            return self.send().await;
        }
        let (client, req) = self.build_split();
        let req = req?;
        let url = req.url();
        let action = url
            .query_pairs()
            .find(|(k, _)| k == "action")
            .map(|(_, v)| format!("?action={}", v))
            .unwrap_or_default();
        // Signed download URLs carry a token in the path too.
        let path = if url.path().starts_with("/sapi/") {
            url.path().to_string()
        } else {
            "/…".to_string()
        };
        let label = format!(
            "{} {}{}{}",
            req.method(),
            url.host_str().unwrap_or("?"),
            path,
            action
        );
        let start = std::time::Instant::now();
        let resp = client.execute(req).await;
        let ms = start.elapsed().as_millis();
        match &resp {
            Ok(r) => eprintln!(
                "[trace #{}] {} → {} in {} ms",
                n,
                label,
                r.status().as_u16(),
                ms
            ),
            Err(e) => eprintln!("[trace #{}] {} → error after {} ms: {}", n, label, ms, e),
        }
        resp
    }
}

/// Generate a random device ID in the format used by the web client:
/// `web-` followed by 32 hex characters.
pub fn generate_device_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut id = String::from("web-");
    for _ in 0..2 {
        // Each `RandomState` is seeded with fresh random keys.
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        id.push_str(&format!("{:016x}", hasher.finish()));
    }
    id
}

/// Generate a device ID in the mobile app's format: `ipad-omh-` followed
/// by the standard base64 of 16 random bytes (on macOS the app presents
/// itself as an iPad). The native (token) login uses this so the session
/// it opens matches what the server links for an app client.
pub fn generate_native_device_id() -> String {
    use base64::Engine;
    let bytes = random_bytes(16);
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("ipad-omh-{}", b64)
}

/// Value of the cookie `name` from the response's `Set-Cookie` headers.
fn set_cookie_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::SET_COOKIE).iter().find_map(|v| {
        let (key, rest) = v.to_str().ok()?.split_once('=')?;
        (key.trim() == name).then(|| rest.split(';').next().unwrap_or("").to_string())
    })
}

/// `n` random bytes from the OS (`/dev/urandom`), with a non-cryptographic
/// fallback so a build without that device still works.
fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut buf = vec![0u8; n];
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n + 8);
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

fn base64url_nopad(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A PKCE pair: the `code_verifier` to keep and the S256 `code_challenge`
/// to send in the authorization request.
fn pkce_pair() -> (String, String) {
    use sha2::{Digest, Sha256};
    let verifier = base64url_nopad(&random_bytes(32)); // 43 unreserved chars
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64url_nopad(&hasher.finalize());
    (verifier, challenge)
}

/// Random UUID-shaped `nonce` for the OIDC authorization request.
fn random_nonce() -> String {
    let b = random_bytes(16);
    format!(
        "{}-{}-{}-{}-{}",
        hex_of(&b[0..4]),
        hex_of(&b[4..6]),
        hex_of(&b[6..8]),
        hex_of(&b[8..10]),
        hex_of(&b[10..16])
    )
    .to_uppercase()
}

/// O2 Cloud API client.
///
/// Holds a `reqwest::Client` with cookie storage enabled so that
/// JSESSIONID cookies are preserved across requests.  Headers are
/// set to mimic the official O2 Cloud desktop app (QtWebEngine).
pub struct O2Client {
    client: Client,
    /// Same cookie jar as `client`, but never follows redirects — the
    /// OAuth login steps need to read the `Location` / `Set-Cookie` of
    /// the 302 responses themselves.
    no_redirect: Client,
    device_id: String,
    /// The folder list `check_session` fetched, handed to the next
    /// `get_all_folders` call instead of asking for it again.
    prefetched_folders: std::sync::Mutex<Option<Vec<FolderEntry>>>,
}

impl O2Client {
    /// Create a new API client with browser-matching defaults. `device_id`
    /// should be the one stored with the session; pass `""` to generate a
    /// new one (e.g. for a fresh login).
    pub fn new(device_id: &str) -> Result<Self, O2Error> {
        // Only headers common to both the web and native identities live
        // here. The browser `User-Agent` is a safe default: the native
        // calls override it cleanly with `APP_USER_AGENT` (reqwest keeps
        // the per-request value and does not duplicate it). `Origin` is
        // *not* a default — see `WEB_ORIGIN` — because the native app
        // sends none and a default header can't be dropped per request.
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 26_5_1) AppleWebKit/537.36 \
                 (KHTML, like Gecko) QtWebEngine/5.15.2 Chrome/83.0.4103.122 Safari/537.36",
            ),
        );
        headers.insert(header::ACCEPT, header::HeaderValue::from_static("*/*"));
        headers.insert(
            header::ACCEPT_LANGUAGE,
            header::HeaderValue::from_static("es"),
        );

        let device_id = if device_id.is_empty() {
            generate_device_id()
        } else {
            device_id.to_string()
        };

        let jar = std::sync::Arc::new(reqwest::cookie::Jar::default());
        let client = Client::builder()
            .cookie_provider(jar.clone())
            .default_headers(headers.clone())
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let no_redirect = Client::builder()
            .cookie_provider(jar)
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            no_redirect,
            device_id,
            prefetched_folders: std::sync::Mutex::new(None),
        })
    }

    /// The device ID sent as `x-deviceid`; persisted with the session.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Initialise the session by fetching the home page, as the web
    /// client does.
    ///
    /// This is required to obtain a `JSESSIONID` cookie from the server
    /// before making any SAPI calls — CloudFront blocks requests that
    /// lack a valid session cookie.
    pub async fn init_session(&self) -> Result<Option<String>, O2Error> {
        let url = format!("{}/", BASE_URL);

        let resp = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .header("x-deviceid", &self.device_id)
            .header(header::ORIGIN, WEB_ORIGIN)
            .header("referer", "https://cloud.o2online.es/")
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty")
            .send_traced()
            .await?;

        // Extract the new JSESSIONID from Set-Cookie
        let new_jsessionid = set_cookie_value(resp.headers(), "JSESSIONID");

        // Drain the response body so the connection is reusable.
        let _ = resp.text().await?;
        Ok(new_jsessionid)
    }

    /// Check whether the stored session is still valid by probing an
    /// authenticated endpoint.  Returns `Ok(false)` only on a 401;
    /// other non-2xx statuses are treated as transient and don't block.
    /// Network failures are returned as errors, so being offline doesn't
    /// trigger a re-login (and its login window).
    pub async fn is_session_valid(&self, auth: &AuthConfig) -> Result<bool, O2Error> {
        let url = format!("{}/sapi/profile/role", BASE_URL);
        let req = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", "get"), ("validationkey", &auth.validationkey)]);
        let resp = self.send_sapi_any_status(req, auth).await?;
        let status = resp.status();
        let _ = resp.text().await?;
        Ok(status != reqwest::StatusCode::UNAUTHORIZED)
    }

    /// Renew the session silently with the stored OAuth token, without a
    /// new SMS code. Posts the token to `/sapi/login/oauth?action=login`,
    /// the same endpoint the first native login uses, and reads the fresh
    /// `JSESSIONID` / `validationKey` from the redirect's cookies.
    ///
    /// The renewal token does not rotate (the server keeps accepting the
    /// same one until the embedded `id_token` expires), so the stored
    /// `access_token` is carried over unchanged.
    pub async fn silent_login(&self, auth: &AuthConfig) -> Result<LoginResponse, O2Error> {
        let auth_header = auth
            .sapi_auth_header()
            .ok_or_else(|| O2Error::Auth("no stored token for silent renewal".into()))?;

        let session = self
            .sapi_oauth_session(&auth_header, Some((&auth.jsessionid, &auth.validationkey)))
            .await?;

        Ok(LoginResponse {
            // The renewal returns the token again (same one, refreshed
            // `lastrefreshdate`); keep the stored one if it is absent.
            access_token: session
                .access_token
                .unwrap_or_else(|| auth.access_token.clone()),
            jsessionid: session.jsessionid,
            validationkey: session.validationkey,
            encryption_token: session
                .encryption_token
                .unwrap_or_else(|| auth.encryption_token.clone()),
        })
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Attach per-request headers that CloudFront / WAF expects from a
    /// browser client: device ID, referer, and Sec-Fetch metadata.  Also
    /// applies `REQUEST_TIMEOUT` — every caller of this helper is a small
    /// metadata/auth exchange, never a raw file transfer.
    fn with_device_headers(
        &self,
        builder: reqwest::RequestBuilder,
        path: &str,
    ) -> reqwest::RequestBuilder {
        builder
            .timeout(REQUEST_TIMEOUT)
            .header("x-deviceid", &self.device_id)
            .header(header::ORIGIN, WEB_ORIGIN)
            .header("referer", format!("https://cloud.o2online.es{}", path))
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty")
    }

    /// Attach headers for SAPI calls (list, upload, download): device ID,
    /// session cookie and, for legacy sessions, the OAuth token.  These
    /// mimic the official desktop client (`omh macos client`) rather
    /// than the web client, because SAPI endpoints reject web-style
    /// Sec-Fetch / Referer headers.
    fn with_sapi_headers(
        &self,
        builder: reqwest::RequestBuilder,
        auth: &AuthConfig,
    ) -> reqwest::RequestBuilder {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let req_id = format!("{:016x}", now);
        let builder = builder
            .header("x-deviceid", &self.device_id)
            .header("x-request-id", req_id)
            .header("cookie", format!("JSESSIONID={}", auth.jsessionid));
        // The web client authenticates with the session cookie and
        // validationkey alone; only token sessions carry an OAuth header,
        // and SAPI serves those the JSON API only under the app's UA.
        //
        // `Origin` goes out on the web session (where the browser sends
        // it) but not on the native token session: the app identifies as
        // `APP_USER_AGENT` and sends no `Origin`, and mixing the two can
        // get the request rejected.
        match auth.sapi_auth_header() {
            Some(h) => builder
                .header(header::USER_AGENT, APP_USER_AGENT)
                .header("authorization", h),
            None => builder.header(header::ORIGIN, WEB_ORIGIN),
        }
    }

    /// Send a SAPI request with `with_sapi_headers`, retrying it while
    /// the server answers 429, or 503 for a request that doesn't create
    /// anything (see `creates_something`). A streamed body (an upload)
    /// can't be replayed, so such a request is sent only once;
    /// `upload_file` retries it itself.
    /// A 401 is `O2Error::SessionExpired`, so a long command (e.g. an
    /// `upload-dir`) can stop instead of failing every remaining file.
    async fn send_sapi(
        &self,
        builder: reqwest::RequestBuilder,
        auth: &AuthConfig,
    ) -> Result<reqwest::Response, O2Error> {
        let resp = self.send_sapi_any_status(builder, auth).await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(O2Error::SessionExpired);
        }
        Ok(resp)
    }

    /// `send_sapi`, returning a 401 like any other status (for the
    /// session checks).
    async fn send_sapi_any_status(
        &self,
        builder: reqwest::RequestBuilder,
        auth: &AuthConfig,
    ) -> Result<reqwest::Response, O2Error> {
        let builder = self.with_sapi_headers(builder, auth);
        let creates = builder
            .try_clone()
            .and_then(|b| b.build().ok())
            .is_some_and(|req| creates_something(&req));
        let mut retry = 0;
        loop {
            let Some(attempt) = builder.try_clone() else {
                return Ok(builder.send_traced().await?);
            };
            let resp = attempt.send_traced().await?;
            if !is_busy(resp.status(), creates) || retry >= BUSY_RETRIES {
                return Ok(resp);
            }
            let wait = busy_wait(&resp, retry);
            retry += 1;
            tokio::time::sleep(wait).await;
        }
    }

    // ------------------------------------------------------------------
    // System Information
    // ------------------------------------------------------------------

    /// Fetch system / server information.
    ///
    /// `GET /sapi/system/information?action=get`
    #[allow(dead_code)]
    pub async fn system_information(&self) -> Result<SystemInfo, O2Error> {
        let url = format!("{}/sapi/system/information", BASE_URL);
        let req = self.client.get(&url).query(&[("action", "get")]);

        let resp = self
            .with_device_headers(req, "/sapi/system/information")
            .send_traced()
            .await?;

        let body: RawResponse<SystemInfo> = resp.json().await?;
        Ok(body.data)
    }

    // ------------------------------------------------------------------
    // Login (OAuth2 + PKCE, handled server-side)
    // ------------------------------------------------------------------

    /// Start the web login flow.
    ///
    /// `GET /sapi/oauth/pkce/authorize?platform=web&deviceid=<id>`
    ///
    /// O2 Cloud generates the PKCE challenge, stores it in the current
    /// session (`JSESSIONID`, from `init_session`) and redirects to the
    /// Telefónica authorization page, whose URL is returned. The user
    /// enters their phone number and SMS code there.
    pub async fn start_oauth_login(&self) -> Result<String, O2Error> {
        let url = format!("{}/sapi/oauth/pkce/authorize", BASE_URL);
        let resp = self
            .no_redirect
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("platform", "web"), ("deviceid", &self.device_id)])
            .header(header::ORIGIN, WEB_ORIGIN)
            .header("referer", "https://cloud.o2online.es/")
            .send_traced()
            .await?;

        let status = resp.status();
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let raw_body = resp.text().await.unwrap_or_default();

        match location {
            Some(loc) if status.is_redirection() && loc.starts_with(AUTHORIZE_URL_PREFIX) => {
                Ok(loc)
            }
            _ => Err(O2Error::Auth(format!(
                "Could not start login (status {}): {}",
                status,
                sapi_error(&raw_body).unwrap_or_else(|| truncate_utf8(&raw_body, 300).into())
            ))),
        }
    }

    /// Finish the web login flow by handing the authorization code from
    /// the Telefónica redirect back to O2 Cloud, in the same session that
    /// started it.
    ///
    /// `GET /sapi/login/oauth?code=...&state=...`
    ///
    /// The server exchanges the code and answers with a redirect that
    /// sets the `JSESSIONID` and `validationKey` cookies.
    pub async fn complete_oauth_login(
        &self,
        code: &str,
        state: &str,
    ) -> Result<SessionTokens, O2Error> {
        let url = format!("{}/sapi/login/oauth", BASE_URL);
        let resp = self
            .no_redirect
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("code", code), ("state", state)])
            .header(header::ORIGIN, WEB_ORIGIN)
            .header("referer", "https://t3.o2online.es/")
            .send_traced()
            .await?;

        let status = resp.status();
        let jsessionid = set_cookie_value(resp.headers(), "JSESSIONID");
        let validationkey = set_cookie_value(resp.headers(), "validationKey");
        let raw_body = resp.text().await.unwrap_or_default();

        match (jsessionid, validationkey) {
            (Some(jsessionid), Some(validationkey)) if !validationkey.is_empty() => {
                Ok(SessionTokens {
                    jsessionid,
                    validationkey,
                })
            }
            _ => Err(O2Error::Auth(format!(
                "Login failed (status {}): {}",
                status,
                sapi_error(&raw_body).unwrap_or_else(|| "no session returned by O2 Cloud".into())
            ))),
        }
    }

    /// Build the authorization URL for the mobile-app OAuth flow, plus the
    /// PKCE verifier needed to complete it. The caller opens the URL in the
    /// login window and intercepts the redirect back to `clientoauth.html`.
    ///
    /// Returns `(authorize_url, code_verifier, state)`.
    pub fn build_oidc_authorize_url(
        &self,
        client_id: &str,
    ) -> Result<(String, String, String), O2Error> {
        let (verifier, challenge) = pkce_pair();
        let state = hex_of(&random_bytes(7)); // 14 hex chars, as the app sends
        let nonce = random_nonce();
        let mut url = url::Url::parse(OIDC_AUTHORIZE_URL)
            .map_err(|e| O2Error::Auth(format!("bad authorize URL: {}", e)))?;
        url.query_pairs_mut()
            .append_pair("state", &state)
            .append_pair("scope", "openid")
            .append_pair("access_type", "offline")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", OIDC_REDIRECT_URI)
            .append_pair("response_type", "code")
            .append_pair("acr_values", "2")
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok((url.to_string(), verifier, state))
    }

    /// Complete the mobile-app OAuth flow: exchange the authorization code
    /// for OIDC tokens at Telefónica (client-side, with the caller-supplied
    /// `client_id`/`client_secret`), then open a SAPI session with them.
    ///
    /// The returned `LoginResponse.access_token` is the token wrapper that
    /// renews the session later without a new SMS code (`silent_login`).
    pub async fn login_native(
        &self,
        code: &str,
        code_verifier: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<LoginResponse, O2Error> {
        // 1. Exchange the code for OIDC tokens at Telefónica.
        let token_resp = self
            .client
            .post(OIDC_TOKEN_URL)
            .timeout(REQUEST_TIMEOUT)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", OIDC_REDIRECT_URI),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code_verifier", code_verifier),
            ])
            .send_traced()
            .await?;
        let status = token_resp.status();
        let raw = token_resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Token exchange failed (status {}): {}",
                status,
                truncate_utf8(&raw, 300)
            )));
        }
        let token: OidcToken = serde_json::from_str(&raw).map_err(|e| {
            O2Error::Auth(format!(
                "Failed to parse token response: {} — body: {}",
                e,
                truncate_utf8(&raw, 300)
            ))
        })?;

        // 2. Wrap the token the way the app does and open a SAPI session.
        //    The server answers with a 302 whose cookies carry the
        //    session; the wrapper is stored as the renewal token so the
        //    session can later renew itself without a new SMS.
        let wrapper = token.sapi_wrapper();
        let session = self
            .sapi_oauth_session(&format!("oauth {}", wrapper), None)
            .await?;
        Ok(LoginResponse {
            // Store the server's enriched token (returned in the 200 body),
            // which is the one renewals must present; fall back to the
            // wrapper only if the response carried none.
            access_token: session.access_token.unwrap_or(wrapper),
            jsessionid: session.jsessionid,
            validationkey: session.validationkey,
            encryption_token: session.encryption_token.unwrap_or_default(),
        })
    }

    /// Open or renew a SAPI session with an `oauth <wrapper>` token by
    /// posting to `/sapi/login/oauth?action=login`. Both the first native
    /// login and the silent renewal use this; the server replies with a
    /// 302 whose `Set-Cookie` carries the new `JSESSIONID` and
    /// `validationKey`, which are returned as `(jsessionid, validationkey)`.
    ///
    /// The non-redirecting client is used on purpose: the default client
    /// would follow the 302 to the web SPA and mask the session as a
    /// `200 text/html` (the `index.html` response).
    async fn sapi_oauth_session(
        &self,
        auth_header: &str,
        existing: Option<(&str, &str)>,
    ) -> Result<SapiSession, O2Error> {
        let url = format!("{}/sapi/login/oauth", BASE_URL);
        // The renewal carries the existing `validationkey` (CSRF guard);
        // the first login has none yet.
        let mut query = vec![("action", "login"), ("responsetime", "true")];
        if let Some((_, vk)) = existing {
            query.push(("validationkey", vk));
        }
        // First login has no session yet, so bootstrap an anonymous
        // `JSESSIONID`. Renewal instead replays the stored (now-expired)
        // cookie, exactly as the app does: the server pairs that cookie
        // with the `validationkey` and the token and issues a fresh
        // session. A new anonymous cookie would not match the stored
        // `validationkey` and the server answers 401.
        if existing.is_none() {
            self.init_session().await?;
        }
        let mut builder = self
            .no_redirect
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&query)
            .header(header::USER_AGENT, APP_USER_AGENT)
            .header("authorization", auth_header)
            .header("x-deviceid", &self.device_id)
            .header("x-devicename", "")
            .header("x-request-id", random_nonce());
        if let Some((jsessionid, _)) = existing {
            builder = builder.header("cookie", format!("JSESSIONID={}", jsessionid));
        }
        let resp = builder.send_traced().await?;

        let status = resp.status();
        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let set_cookie_names: Vec<String> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|c| c.split('=').next().unwrap_or("").trim().to_string())
            .collect();
        let cookie_jsessionid = set_cookie_value(resp.headers(), "JSESSIONID");
        let cookie_validationkey = set_cookie_value(resp.headers(), "validationKey");
        let raw_body = resp.text().await.unwrap_or_default();

        // Two response shapes carry the new session:
        //  - first login: a 302 with `JSESSIONID` + `validationKey` in
        //    `Set-Cookie` (no body);
        //  - renewal: a 200 with a JSON body `{"data":{...}}` holding the
        //    new `jsessionid` / `validationkey` (and a fresh token), plus
        //    `JSESSIONID` in `Set-Cookie`.
        let body: Option<serde_json::Value> = serde_json::from_str(&raw_body).ok();
        let data = body.as_ref().and_then(|b| b.get("data"));
        let field = |name: &str| {
            data.and_then(|d| d.get(name))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let jsessionid = field("jsessionid").or(cookie_jsessionid);
        let validationkey = field("validationkey").or(cookie_validationkey);
        if let (Some(jsessionid), Some(validationkey)) = (jsessionid, validationkey) {
            if !jsessionid.is_empty() && !validationkey.is_empty() {
                return Ok(SapiSession {
                    jsessionid,
                    validationkey,
                    access_token: field("access_token"),
                    encryption_token: field("encryption-token"),
                });
            }
        }
        // No usable session: surface what the server actually returned.
        Err(O2Error::Auth(format!(
            "SAPI login returned no session (status {}, content-type {:?}, \
             location {:?}, set-cookie {:?}): {}",
            status,
            content_type,
            location,
            set_cookie_names,
            sapi_error(&raw_body).unwrap_or_else(|| truncate_utf8(&raw_body, 300).into())
        )))
    }

    /// The account's phone number as 9 national digits, from the profile.
    ///
    /// `GET /sapi/profile?action=get`
    pub async fn get_profile_msisdn(&self, auth: &AuthConfig) -> Result<Option<String>, O2Error> {
        let url = format!("{}/sapi/profile", BASE_URL);
        let req = self
            .client
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", "get"), ("validationkey", &auth.validationkey)]);
        let resp = self.send_sapi(req, auth).await?;
        let raw_body = resp.text().await?;
        let value: serde_json::Value = serde_json::from_str(&raw_body)
            .map_err(|e| O2Error::Parse(format!("Failed to parse profile: {}", e)))?;
        Ok(value["data"]["user"]["phones"]
            .as_array()
            .and_then(|phones| phones.iter().find_map(|p| p["phonenumber"].as_str()))
            .map(|n| n.strip_prefix("34").unwrap_or(n).to_string()))
    }

    // ------------------------------------------------------------------
    // Folder / Media Listing
    // ------------------------------------------------------------------

    /// Get ALL folders in the account.  Returns a flat list; callers
    /// should build the tree by resolving `parentid` links.
    ///
    /// `POST /sapi/media/folder?action=get`
    pub async fn get_all_folders(&self, auth: &AuthConfig) -> Result<Vec<FolderEntry>, O2Error> {
        let prefetched = self
            .prefetched_folders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(folders) = prefetched {
            return Ok(folders);
        }
        let (status, raw) = self.request_folders(auth).await?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(O2Error::SessionExpired);
        }
        parse_folders(status, &raw)
    }

    /// Check the stored session like `is_session_valid`, by asking for the
    /// folder list, which nearly every command asks for anyway: the next
    /// `get_all_folders` call gets it without a request of its own.
    pub async fn check_session(&self, auth: &AuthConfig) -> Result<bool, O2Error> {
        let (status, raw) = self.request_folders(auth).await?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(false);
        }
        if let Ok(folders) = parse_folders(status, &raw) {
            *self
                .prefetched_folders
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(folders);
        }
        Ok(true)
    }

    /// `POST /sapi/media/folder?action=get`: status and body.
    async fn request_folders(
        &self,
        auth: &AuthConfig,
    ) -> Result<(reqwest::StatusCode, String), O2Error> {
        let url = format!("{}/sapi/media/folder", BASE_URL);
        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", "get"), ("validationkey", &auth.validationkey)]);
        let resp = self.send_sapi_any_status(req, auth).await?;
        let status = resp.status();
        Ok((status, resp.text().await?))
    }

    /// Every file in the account. Listing them all takes one request per
    /// 1,000 files, so the list is kept in a local cache (see `cache`) and
    /// only what changed since is asked for; the whole account is listed
    /// when there is no usable cache, or anything about updating it goes
    /// wrong. Download URLs aren't cached: files from the cache have none.
    ///
    /// `POST /sapi/media?action=get` with JSON body specifying fields.
    pub async fn get_all_media(&self, auth: &AuthConfig) -> Result<Vec<MediaItem>, O2Error> {
        // The cache is per account; without a phone number to tell
        // accounts apart, don't use one.
        let account = auth.msisdn.as_str();
        if !crate::cache::enabled() || account.is_empty() {
            return Ok(in_listing_order(self.list_media(None, auth).await?));
        }
        if let Some(cache) = crate::cache::load(account) {
            match self.update_cache(cache, auth).await {
                Ok(media) => return Ok(in_listing_order(media)),
                Err(e) if tracing() => {
                    eprintln!("[trace] cache not updated ({e}); listing everything")
                }
                Err(_) => {}
            }
        }
        // The cursor is taken before listing, so changes made meanwhile
        // are applied again next time rather than missed.
        let cursor = self.changes_since(FAR_FUTURE_MS, auth).await.ok();
        let media = self.list_media(None, auth).await?;
        if let Some(changes) = cursor {
            let cached = media.iter().map(MediaItem::to_cached).collect();
            crate::cache::save(&crate::cache::MediaCache::new(
                account,
                changes.requesttime,
                cached,
            ));
        }
        Ok(in_listing_order(media))
    }

    /// `get_all_media`, after waiting (up to `max_wait`) for the files the
    /// cache knows of but the server doesn't return yet, such as uploads
    /// of the last minute, so they aren't taken for missing. Also returns
    /// how many are still missing.
    pub async fn get_all_media_settled(
        &self,
        auth: &AuthConfig,
        max_wait: Duration,
    ) -> Result<(Vec<MediaItem>, usize), O2Error> {
        let start = std::time::Instant::now();
        let mut said = false;
        loop {
            let media = self.get_all_media(auth).await?;
            let pending = crate::cache::pending_count(&auth.msisdn);
            if pending == 0 || start.elapsed() >= max_wait {
                return Ok((media, pending));
            }
            if !said {
                eprintln!(
                    "→ Waiting for {} file(s) uploaded moments ago to show up in the listing…",
                    pending
                );
                said = true;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// Apply the change log since `cache.cursor` to the cache, save it and
    /// return its files.
    async fn update_cache(
        &self,
        mut cache: crate::cache::MediaCache,
        auth: &AuthConfig,
    ) -> Result<Vec<MediaItem>, O2Error> {
        let changes = self
            .changes_since(cache.cursor.saturating_sub(CHANGES_OVERLAP_MS), auth)
            .await?;
        if let Some(kind) = changes.unknown {
            return Err(O2Error::Api(format!("unknown kind of change '{kind}'")));
        }
        let now = crate::config::now_unix();
        let deleted: std::collections::HashSet<u64> = changes.deleted.iter().copied().collect();
        // Changed files, plus those still pending from last time.
        let mut wanted: Vec<u64> = changes.changed.clone();
        let mut since: std::collections::HashMap<u64, u64> = Default::default();
        for &(id, first) in &cache.pending {
            // Something the server never returns would stay out of the
            // cache for good: list everything instead.
            if now.saturating_sub(first) >= PENDING_FOR_SECS {
                return Err(O2Error::Api(format!("file {id} still not returned")));
            }
            wanted.push(id);
            since.insert(id, first);
        }
        wanted.retain(|id| !deleted.contains(id));
        wanted.sort_unstable();
        wanted.dedup();

        let mut fresh: std::collections::HashMap<u64, crate::cache::CachedItem> =
            Default::default();
        for batch in wanted.chunks(MEDIA_PAGE_SIZE) {
            for item in self.get_media_by_ids(batch, auth).await? {
                fresh.insert(item.id, item.to_cached());
            }
        }
        // Drop deleted files and the old version of changed ones; those
        // the server didn't return stay out until it does.
        let wanted_set: std::collections::HashSet<u64> = wanted.iter().copied().collect();
        cache
            .media
            .retain(|m| !deleted.contains(&m.id) && !wanted_set.contains(&m.id));
        cache.pending = wanted
            .iter()
            .filter(|id| !fresh.contains_key(id))
            .map(|&id| (id, since.get(&id).copied().unwrap_or(now)))
            .collect();
        for id in &wanted {
            if let Some(item) = fresh.remove(id) {
                cache.media.push(item);
            }
        }
        cache.cursor = changes.requesttime;
        crate::cache::save(&cache);
        if tracing() {
            eprintln!(
                "[trace] cache: {} files, {} changed, {} deleted, {} pending",
                cache.media.len(),
                wanted.len(),
                deleted.len(),
                cache.pending.len()
            );
        }
        Ok(cache.media.iter().map(MediaItem::from_cached).collect())
    }

    /// What changed in the account since `from` (Unix ms, server time):
    /// the IDs of files added, changed, moved or renamed, and of files
    /// deleted. Each file appears once, with its latest change.
    ///
    /// `GET /sapi/profile/changes?action=get&from=<ms>&origin=omh,dropbox`
    async fn changes_since(&self, from: u64, auth: &AuthConfig) -> Result<Changes, O2Error> {
        let url = format!("{}/sapi/profile/changes", BASE_URL);
        let req = self.client.get(&url).timeout(REQUEST_TIMEOUT).query(&[
            ("action", "get"),
            ("from", &from.to_string()),
            ("origin", "omh,dropbox"),
            ("validationkey", &auth.validationkey),
        ]);
        let resp = self.send_sapi(req, auth).await?;
        let status = resp.status();
        let raw = resp.text().await?;
        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Get changes failed (status {}): {}",
                status,
                truncate_utf8(&raw, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw) {
            return Err(O2Error::Api(format!("Get changes failed: {}", err)));
        }
        parse_changes(&raw)
    }

    /// Whether the change log reports file `id` deleted since `from`
    /// (Unix ms).
    pub async fn was_deleted_since(
        &self,
        id: u64,
        from: u64,
        auth: &AuthConfig,
    ) -> Result<bool, O2Error> {
        Ok(self.changes_since(from, auth).await?.deleted.contains(&id))
    }

    /// The files directly in folder `folder_id` (not in its subfolders),
    /// without listing the whole account. Photos and videos the phone app
    /// uploads have no folder (`ls` shows them in the root) and are never
    /// returned here, not even for the root folder.
    ///
    /// `POST /sapi/media?action=get&folderid=<id>`
    pub async fn get_folder_media(
        &self,
        folder_id: u64,
        auth: &AuthConfig,
    ) -> Result<Vec<MediaItem>, O2Error> {
        self.list_media(Some(folder_id), auth).await
    }

    /// Every file, or with `folder` the files in that folder, a page of
    /// 1000 at a time.
    async fn list_media(
        &self,
        folder: Option<u64>,
        auth: &AuthConfig,
    ) -> Result<Vec<MediaItem>, O2Error> {
        // The first page alone (a folder's files usually fit in it), then
        // `MEDIA_PAGES_AT_ONCE` pages at a time until one comes back short.
        let mut all = self.media_page(folder, 0, auth).await?;
        let mut full = all.len() == MEDIA_PAGE_SIZE;
        let mut next = 1;
        while full {
            let pages = futures_util::future::try_join_all(
                (next..next + MEDIA_PAGES_AT_ONCE)
                    .map(|page| self.media_page(folder, page * MEDIA_PAGE_SIZE, auth)),
            )
            .await?;
            next += MEDIA_PAGES_AT_ONCE;
            for page in pages {
                if !full {
                    break;
                }
                full = page.len() == MEDIA_PAGE_SIZE;
                all.extend(page);
            }
        }
        Ok(all)
    }

    /// One page of `list_media`: up to `MEDIA_PAGE_SIZE` files from `offset`.
    async fn media_page(
        &self,
        folder: Option<u64>,
        offset: usize,
        auth: &AuthConfig,
    ) -> Result<Vec<MediaItem>, O2Error> {
        #[derive(Deserialize)]
        struct MediaData {
            media: Vec<MediaItem>,
        }

        let url = format!("{}/sapi/media", BASE_URL);
        let payload = serde_json::json!({
            "data": {
                "fields": ["name", "size", "folderid", "url", "creationdate", "modificationdate"]
            }
        });
        let mut query = vec![
            ("action", "get".to_string()),
            ("validationkey", auth.validationkey.clone()),
            ("limit", MEDIA_PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(id) = folder {
            query.push(("folderid", id.to_string()));
        }
        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&query)
            .header("content-type", "application/json")
            .json(&payload);

        let resp = self.send_sapi(req, auth).await?;
        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Get media failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }

        let body: RawResponse<MediaData> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Parse(format!(
                "Failed to parse media list: {} — body: {}",
                e,
                truncate_utf8(&raw_body, 300)
            ))
        })?;
        Ok(body.data.media)
    }

    /// One file, asked for by ID, or `None` if there is no such file. It
    /// sees a new upload a few seconds after it, sooner than the full
    /// listing (`get_all_media`), which can take up to a minute.
    ///
    /// `POST /sapi/media?action=get&origin=omh,dropbox` with
    /// `{"data":{"ids":[<id>],"fields":[...]}}`
    pub async fn get_media_by_id(
        &self,
        media_id: u64,
        auth: &AuthConfig,
    ) -> Result<Option<MediaItem>, O2Error> {
        let media = self.get_media_by_ids(&[media_id], auth).await?;
        Ok(media.into_iter().find(|m| m.id == media_id))
    }

    /// The files with these IDs that exist, in one request (the server
    /// answers 2,000 at once in a fraction of a second; callers send at
    /// most `MEDIA_PAGE_SIZE`). IDs that don't exist are left out.
    pub async fn get_media_by_ids(
        &self,
        media_ids: &[u64],
        auth: &AuthConfig,
    ) -> Result<Vec<MediaItem>, O2Error> {
        #[derive(Deserialize)]
        struct MediaData {
            #[serde(default)]
            media: Vec<MediaItem>,
        }
        let ids: Vec<String> = media_ids.iter().map(|id| id.to_string()).collect();
        let body = serde_json::json!({
            "data": {
                "ids": ids,
                "fields": ["name", "size", "folderid", "url", "creationdate", "modificationdate"]
            }
        });
        let raw = retry_while_validating(|| async {
            let url = format!("{}/sapi/media", BASE_URL);
            let req = self
                .client
                .post(&url)
                .timeout(REQUEST_TIMEOUT)
                .query(&[
                    ("action", "get"),
                    ("origin", "omh,dropbox"),
                    ("validationkey", &auth.validationkey),
                ])
                .json(&body);
            let resp = self.send_sapi(req, auth).await?;
            let status = resp.status();
            let raw = resp.text().await?;
            if !status.is_success() {
                return Err(O2Error::Api(format!(
                    "Get media failed (status {}): {}",
                    status,
                    truncate_utf8(&raw, 300)
                )));
            }
            if let Some(err) = sapi_error(&raw) {
                return Err(O2Error::Api(format!("Get media failed: {}", err)));
            }
            Ok(raw)
        })
        .await?;
        let data: RawResponse<MediaData> = parse_json(&raw, "media")?;
        Ok(data.data.media)
    }

    /// The download URL of one file, asked for by ID. A file uploaded a
    /// moment ago can be listed without a `url` until SAPI has validated
    /// it; this asks again until it has one.
    pub async fn media_download_url(
        &self,
        media_id: u64,
        auth: &AuthConfig,
    ) -> Result<String, O2Error> {
        let mut attempt = 0;
        loop {
            let url = self
                .get_media_by_id(media_id, auth)
                .await?
                .and_then(|m| m.url);
            match url {
                Some(url) => return Ok(url),
                None if attempt < VALIDATION_RETRIES => {
                    attempt += 1;
                    tokio::time::sleep(VALIDATION_RETRY_DELAY).await;
                }
                None => {
                    return Err(O2Error::NotFound(format!(
                        "Media {} has no download URL yet (the server may still be \
                         processing it; try again in a moment)",
                        media_id
                    )))
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Folder Management
    // ------------------------------------------------------------------

    /// Create a new folder.
    ///
    /// `POST /sapi/media/folder?action=save`
    pub async fn create_folder(
        &self,
        name: &str,
        parent_id: u64,
        auth: &AuthConfig,
    ) -> Result<u64, O2Error> {
        if let Some(problem) = folder_name_problem(name) {
            return Err(O2Error::Invalid(format!(
                "can't create folder: {}",
                problem
            )));
        }
        let url = format!("{}/sapi/media/folder", BASE_URL);
        let now = crate::util::system_time_to_compact_iso(std::time::SystemTime::now());

        #[derive(Serialize)]
        struct FolderCreateRequest {
            data: FolderCreateData,
        }
        #[derive(Serialize)]
        struct FolderCreateData {
            creationdate: String,
            magic: bool,
            modificationdate: String,
            name: String,
            parentid: u64,
        }

        let payload = FolderCreateRequest {
            data: FolderCreateData {
                creationdate: now.clone(),
                magic: false,
                modificationdate: now,
                name: name.to_string(),
                parentid: parent_id,
            },
        };

        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", "save"), ("validationkey", &auth.validationkey)])
            .json(&payload);

        let resp = self.send_sapi(req, auth).await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Create folder failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!(
                "Create folder failed: {}{}",
                err,
                limit_hint(&err)
            )));
        }

        #[derive(Deserialize)]
        struct FolderCreateResponse {
            id: u64,
        }

        let body: FolderCreateResponse = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Parse(format!(
                "Failed to parse create folder response: {} — body: {}",
                e,
                truncate_utf8(&raw_body, 200)
            ))
        })?;

        Ok(body.id)
    }

    // ------------------------------------------------------------------
    // Delete
    // ------------------------------------------------------------------

    /// Rename and/or move a folder: `name` is its (new) name and
    /// `parent_id` its (new) parent. The server wants both even when only
    /// one changes, and silently allows two folders with the same name in
    /// one parent, so callers check for that first.
    ///
    /// `POST /sapi/media/folder?action=save` with `{"data":{"id","name","parentid"}}`
    pub async fn update_folder(
        &self,
        folder_id: u64,
        name: &str,
        parent_id: u64,
        auth: &AuthConfig,
    ) -> Result<(), O2Error> {
        if let Some(problem) = folder_name_problem(name) {
            return Err(O2Error::Invalid(format!(
                "can't name the folder so: {}",
                problem
            )));
        }
        let body = serde_json::json!({
            "data": { "id": folder_id, "name": name, "parentid": parent_id }
        });
        self.sapi_post_json("/sapi/media/folder", "save", &body, auth, "Update folder")
            .await?;
        Ok(())
    }

    /// Copy a file on the server into folder `parent_id`, optionally under
    /// a new name; returns the copy's media ID. On a name clash the server
    /// renames the copy (`f (1).txt`), so callers check for that first.
    ///
    /// `POST /sapi/media/file?action=copy` with `{"data":{"id","parentid","name"?}}`
    pub async fn copy_file(
        &self,
        media_id: u64,
        parent_id: u64,
        name: Option<&str>,
        auth: &AuthConfig,
    ) -> Result<u64, O2Error> {
        let mut data = serde_json::json!({ "id": media_id, "parentid": parent_id });
        if let Some(name) = name {
            data["name"] = name.into();
        }
        let body = serde_json::json!({ "data": data });
        let raw = self
            .sapi_post_json("/sapi/media/file", "copy", &body, auth, "Copy")
            .await?;

        #[derive(Deserialize)]
        struct CopyData {
            #[serde(deserialize_with = "deserialize_id")]
            id: u64,
        }
        let body: RawResponse<CopyData> = serde_json::from_str(&raw).map_err(|e| {
            O2Error::Parse(format!(
                "Failed to parse copy response: {} — body: {}",
                e,
                truncate_utf8(&raw, 200)
            ))
        })?;
        Ok(body.data.id)
    }

    /// Storage quota and usage, in bytes.
    ///
    /// `GET /sapi/media?action=get-storage-space&softdeleted=true`
    pub async fn storage_space(&self, auth: &AuthConfig) -> Result<StorageSpace, O2Error> {
        let url = format!("{}/sapi/media", BASE_URL);
        let req = self.client.get(&url).timeout(REQUEST_TIMEOUT).query(&[
            ("action", "get-storage-space"),
            ("softdeleted", "true"),
            ("validationkey", &auth.validationkey),
        ]);
        let resp = self.send_sapi(req, auth).await?;
        let status = resp.status();
        let raw_body = resp.text().await?;
        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Get storage space failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!("Get storage space failed: {}", err)));
        }
        let body: RawResponse<StorageSpace> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Parse(format!(
                "Failed to parse storage space: {} — body: {}",
                e,
                truncate_utf8(&raw_body, 300)
            ))
        })?;
        Ok(body.data)
    }

    /// Soft-delete a folder (move to trash).
    ///
    /// `POST /sapi/media/folder?action=softdelete&id=<id>`
    pub async fn delete_folder(&self, folder_id: u64, auth: &AuthConfig) -> Result<(), O2Error> {
        retry_while_validating(|| self.delete_folder_once(folder_id, auth)).await
    }

    async fn delete_folder_once(&self, folder_id: u64, auth: &AuthConfig) -> Result<(), O2Error> {
        let url = format!("{}/sapi/media/folder", BASE_URL);
        let req = self.client.post(&url).timeout(REQUEST_TIMEOUT).query(&[
            ("action", "softdelete"),
            ("id", &folder_id.to_string()),
            ("validationkey", &auth.validationkey),
        ]);
        let resp = self.send_sapi(req, auth).await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Delete folder failed ({}): {}",
                status,
                truncate_utf8(&body, 200)
            )));
        }
        // The server can refuse with a 200 and an error envelope.
        if let Some(err) = sapi_error(&body) {
            return Err(O2Error::Api(format!("Delete folder failed: {}", err)));
        }
        Ok(())
    }

    /// Soft-delete files (move them to the trash), as the web client
    /// does: one request for a batch of IDs.
    ///
    /// `POST /sapi/media/file?action=delete&softdelete=true`
    /// with body `{"data":{"files":[<id>, ...]}}`
    pub async fn delete_files(&self, media_ids: &[u64], auth: &AuthConfig) -> Result<(), O2Error> {
        retry_while_validating(|| self.delete_files_once(media_ids, auth)).await
    }

    async fn delete_files_once(&self, media_ids: &[u64], auth: &AuthConfig) -> Result<(), O2Error> {
        let url = format!("{}/sapi/media/file", BASE_URL);
        let payload = serde_json::json!({ "data": { "files": media_ids } });

        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[
                ("action", "delete"),
                ("softdelete", "true"),
                ("validationkey", &auth.validationkey),
            ])
            .json(&payload);

        let resp = self.send_sapi(req, auth).await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Delete failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!("Delete failed: {}", err)));
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Upload
    // ------------------------------------------------------------------

    /// Upload a file in one request, as the web client does: a multipart
    /// body with the metadata JSON (`data`) and the file contents (`file`),
    /// streamed from disk. Sends the `meta.size` bytes of `path` that
    /// start at `offset`, so a large file can go up in parts. Returns the
    /// new media ID.
    ///
    /// `POST https://upload.cloud.o2online.es/sapi/upload?action=save`
    ///
    /// No `REQUEST_TIMEOUT`: large files can legitimately take longer.
    pub async fn upload_file(
        &self,
        meta: &UploadMeta,
        path: &std::path::Path,
        offset: u64,
        auth: &AuthConfig,
    ) -> Result<u64, O2Error> {
        let url = format!("{}/sapi/upload", UPLOAD_URL);

        let metadata = serde_json::to_string(&RawRequest { data: meta })
            .map_err(|e| O2Error::Api(format!("Failed to serialize upload metadata: {}", e)))?;

        // The streamed body can't be replayed, so a 429 or 503 is
        // retried here by reading the file again. Either status means the
        // upload was turned away, so a retry can't make a duplicate.
        let mut retry = 0;
        let resp = loop {
            let resp = self
                .send_upload(&url, meta, &metadata, path, offset, auth)
                .await?;
            if !is_busy(resp.status(), false) || retry >= BUSY_RETRIES {
                break resp;
            }
            let wait = busy_wait(&resp, retry);
            retry += 1;
            tokio::time::sleep(wait).await;
        };

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Upload failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!(
                "Upload rejected: {}{}",
                err,
                rejection_hint(&err)
            )));
        }

        let resp_data: UploadResponse = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Parse(format!(
                "Failed to parse upload response: {} — body: {}",
                e,
                truncate_utf8(&raw_body, 300)
            ))
        })?;

        Ok(resp_data.id)
    }

    /// Send one upload request: the metadata `metadata` and the
    /// `meta.size` bytes of `path` from `offset`, streamed from disk.
    async fn send_upload(
        &self,
        url: &str,
        meta: &UploadMeta,
        metadata: &str,
        path: &std::path::Path,
        offset: u64,
        auth: &AuthConfig,
    ) -> Result<reqwest::Response, O2Error> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        let open_err = |e: std::io::Error| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot open {} for upload: {}", path.display(), e),
            ))
        };
        let mut file = tokio::fs::File::open(path).await.map_err(open_err)?;
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(open_err)?;
        }
        let body =
            reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file.take(meta.size)));
        let file_part = reqwest::multipart::Part::stream_with_length(body, meta.size)
            .file_name(meta.name.clone())
            .mime_str(&meta.contenttype)?;
        let form = reqwest::multipart::Form::new()
            .text("data", metadata.to_string())
            .part("file", file_part);

        let req = self
            .client
            .post(url)
            .query(&[("action", "save"), ("validationkey", &auth.validationkey)])
            .header("referer", "https://cloud.o2online.es/")
            .multipart(form);
        self.send_sapi(req, auth).await
    }

    // ------------------------------------------------------------------
    // Sharing, move & rename (requests as sent by the web client)
    // ------------------------------------------------------------------

    /// POST a JSON body to a SAPI endpoint and return the raw response,
    /// failing on a non-2xx status or an error envelope. `what` names the
    /// operation in error messages.
    async fn sapi_post_json(
        &self,
        path: &str,
        action: &str,
        body: &serde_json::Value,
        auth: &AuthConfig,
        what: &str,
    ) -> Result<String, O2Error> {
        retry_while_validating(|| self.sapi_post_json_once(path, action, body, auth, what)).await
    }

    async fn sapi_post_json_once(
        &self,
        path: &str,
        action: &str,
        body: &serde_json::Value,
        auth: &AuthConfig,
        what: &str,
    ) -> Result<String, O2Error> {
        let url = format!("{}{}", BASE_URL, path);
        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", action), ("validationkey", &auth.validationkey)])
            .json(body);
        let resp = self.send_sapi(req, auth).await?;
        let status = resp.status();
        let raw_body = resp.text().await?;
        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "{} failed (status {}): {}",
                what,
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!(
                "{} failed: {}{}",
                what,
                err,
                limit_hint(&err)
            )));
        }
        Ok(raw_body)
    }

    /// Create a public link to a folder.
    ///
    /// `POST /sapi/link/folder?action=save` with `{"data":{"folderid":<id>}}`
    pub async fn share_folder(
        &self,
        folder_id: u64,
        auth: &AuthConfig,
    ) -> Result<SharedLink, O2Error> {
        let body = serde_json::json!({ "data": { "folderid": folder_id } });
        let raw = self
            .sapi_post_json("/sapi/link/folder", "save", &body, auth, "Share folder")
            .await?;
        let resp: RawResponse<SharedLink> = parse_json(&raw, "share folder response")?;
        Ok(resp.data)
    }

    /// Create a public link to a set of files.
    ///
    /// `POST /sapi/media/set?action=save` with `{"data":{"set":{"items":[<id>, ...]}}}`
    pub async fn share_files(
        &self,
        media_ids: &[u64],
        auth: &AuthConfig,
    ) -> Result<SharedLink, O2Error> {
        let body = serde_json::json!({ "data": { "set": { "items": media_ids } } });
        let raw = self
            .sapi_post_json("/sapi/media/set", "save", &body, auth, "Share file")
            .await?;
        // Unlike folder links, the set's id and url are at the top level.
        parse_json(&raw, "share file response")
    }

    /// List the account's public folder links.
    ///
    /// `POST /sapi/link?action=get` with `{"data":{}}`
    pub async fn list_links(&self, auth: &AuthConfig) -> Result<Vec<PublicLink>, O2Error> {
        #[derive(Deserialize)]
        struct LinkList {
            #[serde(default)]
            links: Vec<PublicLink>,
        }
        let body = serde_json::json!({ "data": {} });
        let raw = self
            .sapi_post_json("/sapi/link", "get", &body, auth, "List shared links")
            .await?;
        let resp: RawResponse<LinkList> = parse_json(&raw, "link list")?;
        Ok(resp.data.links)
    }

    /// Delete public links by link ID.
    ///
    /// `POST /sapi/link?action=delete` with `{"data":{"ids":[<id>, ...]}}`
    pub async fn delete_links(&self, link_ids: &[u64], auth: &AuthConfig) -> Result<(), O2Error> {
        let body = serde_json::json!({ "data": { "ids": link_ids } });
        self.sapi_post_json("/sapi/link", "delete", &body, auth, "Unshare")
            .await?;
        Ok(())
    }

    /// Change a file's metadata — `{"name": ...}` to rename it or
    /// `{"folderid": ...}` to move it. `current_name` is the file's name
    /// on the server, which tells which route SAPI wants (see
    /// `media_kind`); if that route answers that it has no such file,
    /// the others are tried.
    ///
    /// `POST /sapi/upload/<kind>?action=save-metadata`, form-encoded
    /// `data={"data":{"id":<id>, ...changes}}`
    pub async fn update_file(
        &self,
        media_id: u64,
        current_name: &str,
        changes: serde_json::Value,
        auth: &AuthConfig,
    ) -> Result<(), O2Error> {
        let new_name = changes["name"].as_str().unwrap_or(current_name);
        if let Some(problem) = file_name_problem(new_name) {
            return Err(O2Error::Invalid(format!(
                "can't name the file so: {}",
                problem
            )));
        }
        // The guesses from the names first, then every other route, in
        // case the server classes an extension differently.
        let mut kinds: Vec<&str> = Vec::new();
        for kind in [
            media_kind(current_name),
            media_kind(new_name),
            "file",
            "picture",
            "video",
            "audio",
        ] {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }

        let mut last_err = None;
        for kind in kinds {
            match retry_while_validating(|| self.update_file_once(kind, media_id, &changes, auth))
                .await
            {
                Ok(()) => return Ok(()),
                // MED-1005 ("Unable to retrieve media with given id") is
                // the answer of a route for another kind of file: try the
                // next. Any other error is the real one.
                Err(e) if matches!(&e, O2Error::Api(msg) if msg.contains("MED-1005")) => {
                    last_err = Some(e)
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| O2Error::Api("Update file failed".into())))
    }

    async fn update_file_once(
        &self,
        kind: &str,
        media_id: u64,
        changes: &serde_json::Value,
        auth: &AuthConfig,
    ) -> Result<(), O2Error> {
        let mut data = serde_json::json!({ "id": media_id });
        if let (Some(obj), Some(changes)) = (data.as_object_mut(), changes.as_object()) {
            obj.extend(changes.clone());
        }
        let form_value = serde_json::json!({ "data": data }).to_string();

        let url = format!("{}/sapi/upload/{}", BASE_URL, kind);
        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[
                ("action", "save-metadata"),
                ("validationkey", &auth.validationkey),
            ])
            .form(&[("data", form_value)]);
        let resp = self.send_sapi(req, auth).await?;
        let status = resp.status();
        let raw_body = resp.text().await?;
        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Update file failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!(
                "Update file failed: {}{}",
                err,
                limit_hint(&err)
            )));
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Download
    // ------------------------------------------------------------------

    /// Download a file from its signed URL (from the `url` field in the
    /// media listing response) and append it to `file` (at `path`, named
    /// in errors), streaming it instead of buffering the whole file in
    /// memory. Returns the byte count written; fails if `expected_size`
    /// is given and a different number of bytes arrives (a cut
    /// download). Leaves removing the partial file to the caller.
    pub async fn download_into(
        &self,
        download_url: &str,
        file: &mut tokio::fs::File,
        path: &std::path::Path,
        expected_size: Option<u64>,
    ) -> Result<u64, O2Error> {
        let resp = self.client.get(download_url).send_traced().await?;

        let status = resp.status();
        if !status.is_success() {
            let raw_body = resp.text().await.unwrap_or_default();
            return Err(O2Error::Api(format!(
                "Download failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 200)
            )));
        }

        // SAPI can answer a download with a 200 and a JSON error instead
        // of the file. A small JSON body is read first to tell that apart
        // from a JSON file of the user's: an error envelope whose length
        // isn't the file's size is an error.
        let is_json = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("json") || ct.contains("javascript"));
        // Without a Content-Length, go by the size the listing gave.
        let small = resp
            .content_length()
            .or(expected_size)
            .is_none_or(|n| n <= 1 << 20);
        let total = if is_json && small {
            let bytes = resp.bytes().await?.to_vec();
            if expected_size != Some(bytes.len() as u64) {
                if let Some(err) = std::str::from_utf8(&bytes).ok().and_then(sapi_error) {
                    return Err(O2Error::Api(format!("Download failed: {}", err)));
                }
            }
            Self::write_bytes_to_file(&bytes, file, path).await?
        } else {
            Self::stream_response_to_file(resp, file, path).await?
        };
        match expected_size {
            Some(size) if size != total => Err(O2Error::Api(format!(
                "Download incomplete: got {} of {} bytes",
                total, size
            ))),
            _ => Ok(total),
        }
    }

    /// Download one file into `file` from position `base` on, where an
    /// earlier attempt left its first `offset` bytes (so `file` is
    /// `base + offset` long). The rest is asked for with a `Range` request
    /// (O2 answers 206 to it, checked live) and appended; if the server
    /// sends the whole file instead, `file` is cut back to `base` and it is
    /// written from the start. Returns the file's length (`offset` plus
    /// what arrived); fails if that isn't `expected_size`.
    pub async fn download_resuming(
        &self,
        download_url: &str,
        file: &mut tokio::fs::File,
        path: &std::path::Path,
        base: u64,
        offset: u64,
        expected_size: Option<u64>,
    ) -> Result<u64, O2Error> {
        use tokio::io::AsyncSeekExt;
        if expected_size == Some(offset) && offset > 0 {
            return Ok(offset);
        }
        if offset == 0 {
            file.set_len(base).await?;
            file.seek(std::io::SeekFrom::Start(base)).await?;
            return self
                .download_into(download_url, file, path, expected_size)
                .await;
        }
        let resp = self
            .client
            .get(download_url)
            .header(header::RANGE, format!("bytes={}-", offset))
            .send_traced()
            .await?;
        let status = resp.status();
        let total = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            file.seek(std::io::SeekFrom::Start(base + offset)).await?;
            offset + Self::stream_response_to_file(resp, file, path).await?
        } else if status.is_success() {
            drop(resp);
            file.set_len(base).await?;
            file.seek(std::io::SeekFrom::Start(base)).await?;
            return self
                .download_into(download_url, file, path, expected_size)
                .await;
        } else {
            let raw_body = resp.text().await.unwrap_or_default();
            return Err(O2Error::Api(format!(
                "Download failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 200)
            )));
        };
        match expected_size {
            Some(size) if size != total => Err(O2Error::Api(format!(
                "Download incomplete: got {} of {} bytes",
                total, size
            ))),
            _ => Ok(total),
        }
    }

    async fn write_bytes_to_file(
        bytes: &[u8],
        file: &mut tokio::fs::File,
        path: &std::path::Path,
    ) -> Result<u64, O2Error> {
        use tokio::io::AsyncWriteExt;

        file.write_all(bytes).await.map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot write to {}: {}", path.display(), e),
            ))
        })?;
        file.flush().await.map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot flush {}: {}", path.display(), e),
            ))
        })?;
        Ok(bytes.len() as u64)
    }

    async fn stream_response_to_file(
        resp: reqwest::Response,
        file: &mut tokio::fs::File,
        path: &std::path::Path,
    ) -> Result<u64, O2Error> {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;

        let mut stream = resp.bytes_stream();
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await.map_err(|e| {
                O2Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("Cannot write to {}: {}", path.display(), e),
                ))
            })?;
            total += chunk.len() as u64;
        }
        file.flush().await.map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot flush {}: {}", path.display(), e),
            ))
        })?;

        Ok(total)
    }
}

/// Create `path` for writing, failing if it already exists, so an
/// existing local file is never overwritten (nor removed by the cleanup
/// after a failed download).
pub async fn create_new_file(path: &std::path::Path) -> Result<tokio::fs::File, O2Error> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot create {}: {}", path.display(), e),
            ))
        })
}

// ======================================================================
// Request / Response Types
// ======================================================================

/// Generic wrapper used by the SAPI: `{"data": { ... }}`
#[derive(Debug, Deserialize)]
struct RawResponse<T> {
    data: T,
}

/// Generic request wrapper: `{"data": { ... }}`
#[derive(Debug, Serialize)]
struct RawRequest<T: Serialize> {
    data: T,
}

// -- System Information ------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SystemInfo {
    #[serde(rename = "sapiversion")]
    pub sapi_version: String,
    #[serde(rename = "production-environment")]
    pub production_environment: String,
    pub logininfo: Option<LoginInfo>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LoginInfo {
    pub supportedtypes: Option<String>,
    pub oauthinfo: Option<OAuthInfo>,
    pub loginfields: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OAuthInfo {
    pub enabled: Option<bool>,
    pub authorizationcodeurl: Option<String>,
    pub accesstokenurl: Option<String>,
}

// -- OAuth login -------------------------------------------------------

/// Session cookies returned by the OAuth login callback.
#[derive(Debug)]
pub struct SessionTokens {
    pub jsessionid: String,
    pub validationkey: String,
}

// -- Login -------------------------------------------------------------

/// OIDC token response from Telefónica's `/token` endpoint (mobile-app
/// OAuth flow).
#[derive(Debug, Deserialize)]
struct OidcToken {
    access_token: String,
    #[serde(default)]
    expires_in: serde_json::Value,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
}

impl OidcToken {
    /// Base64 of the `{"data":{...}}` JSON the app sends as the
    /// `authorization: oauth <...>` value to `/sapi/login/oauth`.
    fn sapi_wrapper(&self) -> String {
        use base64::Engine;
        let expires_in = match &self.expires_in {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => "3600".to_string(),
        };
        // The first-login token the app presents carries only the raw
        // OIDC fields plus `platform` and `lastrefreshdate: 0`, in this
        // order. The `0` marks it as never-refreshed, so the server
        // answers 200 with the real, enriched token (tokentype/scope/
        // keepmelogged and a live lastrefreshdate) to store for later
        // renewals. Presenting the full renewal shape here instead makes
        // the server answer 302 without a token in the body. A struct
        // (not `json!`) keeps the fields in the app's order.
        #[derive(Serialize)]
        struct FirstLoginData<'a> {
            refreshtoken: &'a str,
            accesstoken: &'a str,
            id_token: &'a str,
            expiresin: String,
            platform: &'a str,
            lastrefreshdate: u8,
        }
        #[derive(Serialize)]
        struct FirstLoginPayload<'a> {
            data: FirstLoginData<'a>,
        }
        let payload = FirstLoginPayload {
            data: FirstLoginData {
                refreshtoken: &self.refresh_token,
                accesstoken: &self.access_token,
                id_token: &self.id_token,
                expiresin: expires_in,
                platform: "ios",
                lastrefreshdate: 0,
            },
        };
        let json = serde_json::to_string(&payload).unwrap_or_default();
        base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct LoginResponse {
    pub access_token: String,
    pub jsessionid: String,
    pub validationkey: String,
    pub encryption_token: String,
}

/// The session a `/sapi/login/oauth` call hands back. `access_token` and
/// `encryption_token` are only present on the renewal (JSON body) shape;
/// the first-login (cookie) shape leaves them `None`.
struct SapiSession {
    jsessionid: String,
    validationkey: String,
    access_token: Option<String>,
    encryption_token: Option<String>,
}

// -- Folder / Media Listing -------------------------------------------

/// A folder entry returned by `POST /sapi/media/folder?action=get`.
#[derive(Debug, Deserialize)]
pub struct FolderEntry {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: u64,
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_id")]
    pub parentid: u64,
}

/// Storage figures from `get-storage-space`, in bytes.
#[derive(Debug, Deserialize, Serialize)]
pub struct StorageSpace {
    pub quota: u64,
    pub used: u64,
    pub free: u64,
    /// Bytes in the trash.
    #[serde(default)]
    pub softdeleted: u64,
    /// The plan has no storage limit.
    #[serde(default)]
    pub nolimit: bool,
}

/// A media item returned by `POST /sapi/media?action=get`.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaItem {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    /// The API returns this as `folder` (number) even when we ask for `folderid`.
    #[serde(
        default,
        alias = "folder",
        deserialize_with = "deserialize_optional_id"
    )]
    pub folderid: Option<u64>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
    /// When the file was uploaded, in Unix milliseconds. Sent even when
    /// not asked for (asking for it is rejected).
    #[serde(default, deserialize_with = "deserialize_optional_timestamp")]
    pub date: Option<u64>,
    /// When the file itself was created (for a photo, when it was taken),
    /// in Unix milliseconds.
    #[serde(default, deserialize_with = "deserialize_optional_timestamp")]
    pub creationdate: Option<u64>,
    /// The local file's modification date sent when it was uploaded, in
    /// Unix milliseconds (0 for one before 1970). Unlike `creationdate`,
    /// which O2 replaces with a photo's EXIF date, it is kept as sent
    /// (checked live), so it is the date to give a downloaded file.
    #[serde(default, deserialize_with = "deserialize_optional_timestamp")]
    pub modificationdate: Option<u64>,
}

impl MediaItem {
    fn to_cached(&self) -> crate::cache::CachedItem {
        crate::cache::CachedItem {
            id: self.id,
            name: self.name.clone(),
            folderid: self.folderid,
            size: self.size,
            date: self.date,
            creationdate: self.creationdate,
            modificationdate: self.modificationdate,
        }
    }

    /// A file from the cache. It has no download URL (they expire), so
    /// downloads ask for one.
    fn from_cached(item: &crate::cache::CachedItem) -> Self {
        MediaItem {
            id: item.id,
            name: item.name.clone(),
            folderid: item.folderid,
            size: item.size,
            url: None,
            date: item.date,
            creationdate: item.creationdate,
            modificationdate: item.modificationdate,
        }
    }
}

/// Unix-millisecond timestamp. O2 sends negative values for some old
/// media; read those as unknown instead of failing the whole listing.
fn deserialize_optional_timestamp<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<i64>::deserialize(deserializer)?.and_then(|ms| u64::try_from(ms).ok()))
}

fn deserialize_optional_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    struct OptionalIdVisitor;
    impl de::Visitor<'_> for OptionalIdVisitor {
        type Value = Option<u64>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a number or string id, or null")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Option<u64>, E> {
            Ok(Some(v))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Option<u64>, E> {
            v.parse().map(Some).map_err(de::Error::custom)
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<u64>, E> {
            Ok(None)
        }
    }
    deserializer.deserialize_any(OptionalIdVisitor)
}

fn deserialize_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    struct IdVisitor;
    impl de::Visitor<'_> for IdVisitor {
        type Value = u64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a number or string representing an id")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
            v.parse().map_err(de::Error::custom)
        }
        fn visit_none<E: de::Error>(self) -> Result<u64, E> {
            Ok(0) // field was absent → default
        }
    }
    deserializer.deserialize_any(IdVisitor)
}

// -- Upload ------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct UploadMeta {
    pub contenttype: String,
    pub creationdate: String,
    pub folderid: u64,
    pub modificationdate: String,
    pub name: String,
    pub size: u64,
}

/// A newly created public link (folder link or file set).
#[derive(Debug, Deserialize)]
pub struct SharedLink {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: u64,
    pub url: String,
}

/// A public folder link, as listed by `/sapi/link?action=get`.
#[derive(Debug, Deserialize)]
pub struct PublicLink {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: u64,
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub folderid: Option<u64>,
    #[serde(default)]
    pub foldername: Option<String>,
    pub url: String,
    #[serde(default)]
    pub usages: Option<u64>,
}

/// Parse a JSON response body, naming `what` in the error.
fn parse_json<T: serde::de::DeserializeOwned>(raw: &str, what: &str) -> Result<T, O2Error> {
    serde_json::from_str(raw).map_err(|e| {
        O2Error::Parse(format!(
            "Failed to parse {}: {} — body: {}",
            what,
            e,
            truncate_utf8(raw, 300)
        ))
    })
}

/// Response to an upload: `{"success": "...", "id": "1539028003", ...}`.
#[derive(Debug, Deserialize)]
struct UploadResponse {
    #[serde(deserialize_with = "deserialize_id")]
    id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_device_id_matches_web_format() {
        let a = generate_device_id();
        let b = generate_device_id();
        let hex = a.strip_prefix("web-").expect("web- prefix");
        assert_eq!(hex.len(), 32, "{a}");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn test_set_cookie_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        for v in [
            "JSESSIONID=abc.node1; Max-Age=86400; Domain=cloud.o2online.es; Path=/; Secure; HttpOnly",
            "validationKey=0123456789abcdef; Path=/; Secure",
        ] {
            headers.append(header::SET_COOKIE, v.parse().unwrap());
        }
        assert_eq!(
            set_cookie_value(&headers, "JSESSIONID").as_deref(),
            Some("abc.node1")
        );
        assert_eq!(
            set_cookie_value(&headers, "validationKey").as_deref(),
            Some("0123456789abcdef")
        );
        assert_eq!(set_cookie_value(&headers, "validationkey"), None);
    }

    #[test]
    fn test_parse_share_and_link_responses() {
        // Shapes from captured web traffic (tokens shortened).
        let folder: RawResponse<SharedLink> = parse_json(
            r#"{"data":{"id":133707,"success":"Public link created successfully","url":"https://cloud.o2online.es/share/f/abc"},"responsetime":1}"#,
            "t",
        )
        .unwrap();
        assert_eq!(folder.data.id, 133707);
        assert_eq!(folder.data.url, "https://cloud.o2online.es/share/f/abc");

        let set: SharedLink = parse_json(
            r#"{"success":"Media set saved successfully","id":"5282331","url":"https://cloud.o2online.es/share/xyz","responsetime":1}"#,
            "t",
        )
        .unwrap();
        assert_eq!(set.id, 5282331);

        let link: PublicLink = serde_json::from_str(
            r#"{"owner":{"userid":"u"},"id":133708,"name":"CSVs","allowchanges":false,"usages":0,"password":false,"folderid":35559600,"subfolderaccess":true,"foldername":"CSVs","url":"https://cloud.o2online.es/share/f/abc"}"#,
        )
        .unwrap();
        assert_eq!(link.folderid, Some(35559600));
        assert_eq!(link.usages, Some(0));
    }

    #[test]
    fn test_media_item_negative_timestamp_is_unknown() {
        // Some old phone videos come back with a creation date before 1970.
        let item: MediaItem = serde_json::from_str(
            r#"{"id":"100","name":"VIDEO0001.3gp","date":1700000000000,"creationdate":-777523141000}"#,
        )
        .unwrap();
        assert_eq!(item.creationdate, None);
        assert_eq!(item.date, Some(1700000000000));

        let item: MediaItem =
            serde_json::from_str(r#"{"id":1,"date":null,"creationdate":0}"#).unwrap();
        assert_eq!(item.date, None);
        assert_eq!(item.creationdate, Some(0));

        let item: MediaItem = serde_json::from_str(r#"{"id":1}"#).unwrap();
        assert_eq!(item.date, None);
        assert_eq!(item.creationdate, None);
    }

    #[test]
    fn test_rejection_hint() {
        assert!(
            rejection_hint("The file extension or the format is not supported. (MED-1050)")
                .contains("checks the content")
        );
        assert!(
            rejection_hint("Unknown exception in media handling (MED-1000)").contains("damaged")
        );
        assert_eq!(rejection_hint("Quota exceeded (MED-1004)"), "");
    }

    #[test]
    fn test_parse_changes() {
        // Shape of a real response (IDs shortened)
        let raw = r#"{"requesttime":1795046518451,"responsetime":1795046518813,"data":{
            "picture":{"U":[11,12],"D":[13]},"video":{"D":[14]},"file":{"N":[15],"U":["16"]},
            "folder":{"N":[1],"D":[2,3]},"label":{"N":[4],"U":[5],"D":[6]},"tag":{"U":[7]},
            "mediaset":{"U":[8]},"changes":[{"type":"profile"},{"type":"feature"}]}}"#;
        let c = parse_changes(raw).unwrap();
        assert_eq!(c.requesttime, 1795046518451);
        let mut changed = c.changed.clone();
        changed.sort_unstable();
        assert_eq!(changed, [11, 12, 15, 16]);
        let mut deleted = c.deleted.clone();
        deleted.sort_unstable();
        assert_eq!(deleted, [13, 14]);
        assert_eq!(c.unknown, None);

        // Nothing changed: no `data` at all
        let c = parse_changes(r#"{"requesttime":5,"responsetime":6}"#).unwrap();
        assert_eq!((c.requesttime, c.changed.len(), c.deleted.len()), (5, 0, 0));

        // A kind or operation it doesn't know is reported, not ignored
        let c = parse_changes(r#"{"requesttime":5,"data":{"ebook":{"U":[1]}}}"#).unwrap();
        assert_eq!(c.unknown.as_deref(), Some("ebook"));
        let c = parse_changes(r#"{"requesttime":5,"data":{"file":{"X":[1]}}}"#).unwrap();
        assert_eq!(c.unknown.as_deref(), Some("file:X"));

        assert!(parse_changes(r#"{"data":{}}"#).is_err());
        assert!(parse_changes("<html>").is_err());
    }

    #[test]
    fn test_media_item_cache_round_trip() {
        let item: MediaItem = serde_json::from_str(
            r#"{"id":"7","name":"a.jpg","folder":3,"size":10,"url":"https://x/y",
                "date":1700000000000,"creationdate":1600000000000,"etag":"e"}"#,
        )
        .unwrap();
        let cached = item.to_cached();
        let json = serde_json::to_string(&cached).unwrap();
        let back = MediaItem::from_cached(&serde_json::from_str(&json).unwrap());
        assert_eq!(
            (back.id, back.name.as_deref(), back.folderid, back.size),
            (7, Some("a.jpg"), Some(3), Some(10))
        );
        assert_eq!(
            (back.date, back.creationdate),
            (item.date, item.creationdate)
        );
        // Download URLs expire, so they aren't cached
        assert_eq!(back.url, None);
    }

    #[test]
    fn test_media_kind() {
        assert_eq!(media_kind("song.MP3"), "audio");
        assert_eq!(media_kind("clip.mp4"), "video");
        assert_eq!(media_kind("IMG_0001.JPG"), "picture");
        assert_eq!(media_kind("movie.mkv"), "file");
        assert_eq!(media_kind("notes.pdf"), "file");
        assert_eq!(media_kind("README"), "file");
    }

    #[test]
    fn test_busy_and_creates() {
        let client = reqwest::Client::new();
        let req = |action: &str| {
            client
                .post("https://example.com/sapi/x")
                .query(&[("action", action)])
                .build()
                .unwrap()
        };
        assert!(creates_something(&req("save")));
        assert!(creates_something(&req("copy")));
        assert!(!creates_something(&req("get")));
        assert!(!creates_something(&req("softdelete")));

        let (busy, unavailable) = (
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        );
        assert!(is_busy(busy, true));
        assert!(is_busy(unavailable, false));
        assert!(!is_busy(unavailable, true));
        assert!(!is_busy(reqwest::StatusCode::OK, false));
    }

    #[test]
    fn test_is_validating() {
        let raw = r#"{"error":{"code":"MED-1017","message":"Media not validated"}}"#;
        let err = O2Error::Api(format!("Delete failed: {}", sapi_error(raw).unwrap()));
        assert!(is_validating(&err));
        assert!(!is_validating(&O2Error::Api(
            "Delete failed: (MED-1022)".into()
        )));
        assert!(!is_validating(&O2Error::NotFound("MED-1017".into())));
    }

    #[tokio::test]
    async fn test_retry_while_validating_stops_on_other_errors() {
        let mut calls = 0;
        let result: Result<(), O2Error> = retry_while_validating(|| {
            calls += 1;
            async { Err(O2Error::Api("Update file failed: (MED-1005)".into())) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn test_sapi_error() {
        let raw = r#"{"error":{"code":"PAPI-0000","message":"Unknown exception","parameters":[],"cause":""},"responsetime":1784797653007}"#;
        assert_eq!(
            sapi_error(raw).as_deref(),
            Some("Unknown exception (PAPI-0000)")
        );
        let raw = r#"{"error":{"message":"bad","parameters":[{"param":"name"}]}}"#;
        assert_eq!(sapi_error(raw).as_deref(), Some("bad [params: name]"));
        assert_eq!(sapi_error(r#"{"data":{"authorizationurl":"x"}}"#), None);
        assert_eq!(sapi_error("not json"), None);
    }

    #[test]
    fn test_truncate_utf8_short_string_unchanged() {
        assert_eq!(truncate_utf8("hello", 300), "hello");
    }

    #[test]
    fn test_folder_entry_deserialize_numeric_id() {
        let folder: FolderEntry =
            serde_json::from_str(r#"{"id": 42, "name": "docs", "parentid": 1}"#).unwrap();
        assert_eq!(folder.id, 42);
        assert_eq!(folder.parentid, 1);
    }

    #[test]
    fn test_folder_entry_deserialize_string_id() {
        let folder: FolderEntry =
            serde_json::from_str(r#"{"id": "42", "name": "docs", "parentid": "1"}"#).unwrap();
        assert_eq!(folder.id, 42);
        assert_eq!(folder.parentid, 1);
    }

    #[test]
    fn test_media_item_deserialize_folderid_numeric() {
        let item: MediaItem =
            serde_json::from_str(r#"{"id": 100, "name": "a.txt", "folderid": 5, "size": 10}"#)
                .unwrap();
        assert_eq!(item.id, 100);
        assert_eq!(item.folderid, Some(5));
    }

    #[test]
    fn test_media_item_deserialize_folder_alias_as_string() {
        // The server actually returns `folder`, not `folderid`, even
        // though the request asks for the `folderid` field.
        let item: MediaItem =
            serde_json::from_str(r#"{"id": 101, "name": "b.txt", "folder": "7", "size": 20}"#)
                .unwrap();
        assert_eq!(item.folderid, Some(7));
    }

    #[test]
    fn test_media_item_deserialize_folderid_absent() {
        let item: MediaItem = serde_json::from_str(r#"{"id": 102, "name": "c.txt"}"#).unwrap();
        assert_eq!(item.folderid, None);
    }

    #[test]
    fn test_truncate_utf8_does_not_split_multibyte_char() {
        // "á" is 2 bytes in UTF-8 (0xC3 0xA1). Repeating it 200 times gives
        // a 400-byte string where byte offset 300 falls squarely inside a
        // character boundary (300 is even, so it lands mid-character only
        // if the previous naive byte-slice logic was used incorrectly);
        // build a string where the boundary genuinely falls mid-char by
        // prefixing with a single-byte character to shift alignment.
        let s = format!("x{}", "á".repeat(200));
        // Byte 300 in this string must fall inside a 2-byte 'á' sequence.
        assert!(!s.is_char_boundary(300));
        let truncated = truncate_utf8(&s, 300);
        assert!(s.starts_with(truncated));
        assert!(truncated.len() <= 300);
        // Must not panic and must be valid UTF-8 (guaranteed by &str type).
    }
}
