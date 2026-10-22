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

/// Value of the cookie `name` from the response's `Set-Cookie` headers.
fn set_cookie_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::SET_COOKIE).iter().find_map(|v| {
        let (key, rest) = v.to_str().ok()?.split_once('=')?;
        (key.trim() == name).then(|| rest.split(';').next().unwrap_or("").to_string())
    })
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
}

impl O2Client {
    /// Create a new API client with browser-matching defaults. `device_id`
    /// should be the one stored with the session; pass `""` to generate a
    /// new one (e.g. for a fresh login).
    pub fn new(device_id: &str) -> Result<Self, O2Error> {
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
        headers.insert(
            header::ORIGIN,
            header::HeaderValue::from_static("https://cloud.o2online.es"),
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
            .header("referer", "https://cloud.o2online.es/")
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty")
            .send()
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
        let resp = self.with_sapi_headers(req, auth).send().await?;
        let status = resp.status();
        let _ = resp.text().await?;
        Ok(status != reqwest::StatusCode::UNAUTHORIZED)
    }

    /// Attempt a silent re-login using the stored `access_token` (pat token).
    /// Only sessions from the old Mobile Connect login have one; the
    /// current OAuth login doesn't hand a token to the client.
    /// First gets a fresh JSESSIONID cookie, then calls the login endpoint.
    /// If successful, returns new `LoginResponse` with fresh tokens.
    pub async fn silent_login(&self, auth: &AuthConfig) -> Result<LoginResponse, O2Error> {
        let auth_header = auth
            .sapi_auth_header()
            .ok_or_else(|| O2Error::Auth("no stored token for silent renewal".into()))?;

        // Get a fresh session cookie first
        self.init_session().await?;

        let url = format!("{}/sapi/login", BASE_URL);

        // Try with SAPI auth header (pat token) — this is what the
        // desktop app uses for all its API calls
        let resp = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", "login"), ("responsetime", "true")])
            .header("authorization", &auth_header)
            .header(
                "content-type",
                "application/x-www-form-urlencoded; charset=UTF-8",
            )
            .header("x-deviceid", &self.device_id)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Silent login failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }

        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Auth(format!("Login failed: {}", err)));
        }

        let body: RawResponse<LoginData> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Auth(format!(
                "Failed to parse silent login response: {} — body: {}",
                e,
                truncate_utf8(&raw_body, 300)
            ))
        })?;

        Ok(LoginResponse {
            access_token: body.data.access_token,
            jsessionid: body.data.jsessionid,
            validationkey: body.data.validationkey,
            encryption_token: body.data.encryption_token,
            roles: body.data.roles,
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
        // validationkey alone; only legacy sessions carry an OAuth token.
        match auth.sapi_auth_header() {
            Some(h) => builder.header("authorization", h),
            None => builder,
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
            .send()
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
            .header("referer", "https://cloud.o2online.es/")
            .send()
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
            .header("referer", "https://t3.o2online.es/")
            .send()
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
        let resp = self.with_sapi_headers(req, auth).send().await?;
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
        let url = format!("{}/sapi/media/folder", BASE_URL);

        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", "get"), ("validationkey", &auth.validationkey)]);

        let resp = self.with_sapi_headers(req, auth).send().await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Get folders failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
            )));
        }

        #[derive(Deserialize)]
        struct FolderData {
            folders: Vec<FolderEntry>,
        }

        let body: RawResponse<FolderData> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Parse(format!(
                "Failed to parse folder list: {} — body: {}",
                e,
                truncate_utf8(&raw_body, 300)
            ))
        })?;

        Ok(body.data.folders)
    }

    /// Get all media items with requested fields.  Pages through results
    /// automatically (1000 items per request).
    ///
    /// `POST /sapi/media?action=get` with JSON body specifying fields.
    pub async fn get_all_media(&self, auth: &AuthConfig) -> Result<Vec<MediaItem>, O2Error> {
        let url = format!("{}/sapi/media", BASE_URL);
        let page_size = 1000u64;
        let mut all = Vec::new();
        let mut offset = 0u64;

        #[derive(Serialize)]
        struct MediaRequest {
            data: MediaRequestData,
        }
        #[derive(Serialize)]
        struct MediaRequestData {
            fields: Vec<&'static str>,
        }

        #[derive(Deserialize)]
        struct MediaData {
            media: Vec<MediaItem>,
        }

        loop {
            let payload = MediaRequest {
                data: MediaRequestData {
                    fields: vec!["name", "size", "etag", "folderid", "url"],
                },
            };

            let req = self
                .client
                .post(&url)
                .timeout(REQUEST_TIMEOUT)
                .query(&[
                    ("action", "get"),
                    ("validationkey", &auth.validationkey),
                    ("limit", &page_size.to_string()),
                    ("offset", &offset.to_string()),
                ])
                .header("content-type", "application/json")
                .json(&payload);

            let resp = self.with_sapi_headers(req, auth).send().await?;

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

            let count = body.data.media.len();
            all.extend(body.data.media);
            if count < page_size as usize {
                break;
            }
            offset += page_size;
        }

        Ok(all)
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

        let resp = self.with_sapi_headers(req, auth).send().await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Api(format!(
                "Create folder failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 300)
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

    /// Soft-delete a folder (move to trash).
    ///
    /// `POST /sapi/media/folder?action=softdelete&id=<id>`
    pub async fn delete_folder(&self, folder_id: u64, auth: &AuthConfig) -> Result<(), O2Error> {
        let url = format!("{}/sapi/media/folder", BASE_URL);
        let req = self.client.post(&url).timeout(REQUEST_TIMEOUT).query(&[
            ("action", "softdelete"),
            ("id", &folder_id.to_string()),
            ("validationkey", &auth.validationkey),
        ]);
        let resp = self.with_sapi_headers(req, auth).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(O2Error::Api(format!(
                "Delete folder failed ({}): {}",
                status,
                truncate_utf8(&body, 200)
            )));
        }
        Ok(())
    }

    /// Soft-delete files (move them to the trash), as the web client
    /// does: one request for a batch of IDs.
    ///
    /// `POST /sapi/media/file?action=delete&softdelete=true`
    /// with body `{"data":{"files":[<id>, ...]}}`
    pub async fn delete_files(&self, media_ids: &[u64], auth: &AuthConfig) -> Result<(), O2Error> {
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

        let resp = self.with_sapi_headers(req, auth).send().await?;

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
    /// streamed from disk. Returns the new media ID.
    ///
    /// `POST https://upload.cloud.o2online.es/sapi/upload?action=save`
    ///
    /// No `REQUEST_TIMEOUT`: large files can legitimately take longer.
    pub async fn upload_file(
        &self,
        meta: &UploadMeta,
        path: &std::path::Path,
        auth: &AuthConfig,
    ) -> Result<u64, O2Error> {
        let url = format!("{}/sapi/upload", UPLOAD_URL);

        let metadata = serde_json::to_string(&RawRequest { data: meta })
            .map_err(|e| O2Error::Api(format!("Failed to serialize upload metadata: {}", e)))?;

        let file = tokio::fs::File::open(path).await.map_err(|e| {
            O2Error::Io(std::io::Error::new(
                e.kind(),
                format!("Cannot open {} for upload: {}", path.display(), e),
            ))
        })?;
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        let file_part = reqwest::multipart::Part::stream_with_length(body, meta.size)
            .file_name(meta.name.clone())
            .mime_str(&meta.contenttype)?;
        let form = reqwest::multipart::Form::new()
            .text("data", metadata)
            .part("file", file_part);

        let req = self
            .client
            .post(&url)
            .query(&[("action", "save"), ("validationkey", &auth.validationkey)])
            .header("referer", "https://cloud.o2online.es/")
            .multipart(form);

        let resp = self.with_sapi_headers(req, auth).send().await?;

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
            return Err(O2Error::Api(format!("Upload rejected: {}", err)));
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

    // ------------------------------------------------------------------
    // Download
    // ------------------------------------------------------------------

    /// Download a file from its signed URL (from the `url` field in the
    /// media listing response), streaming it directly to `path` instead
    /// of buffering the whole file in memory. Returns the byte count
    /// written. Fails if `path` already exists. On any error after the
    /// destination file is created, the partially-written file is removed.
    pub async fn download_to_path(
        &self,
        download_url: &str,
        path: &std::path::Path,
    ) -> Result<u64, O2Error> {
        let resp = self.client.get(download_url).send().await?;

        let status = resp.status();
        if !status.is_success() {
            let raw_body = resp.text().await.unwrap_or_default();
            return Err(O2Error::Api(format!(
                "Download failed (status {}): {}",
                status,
                truncate_utf8(&raw_body, 200)
            )));
        }

        // `create_new` fails if the file already exists, so an existing
        // local file is never overwritten (nor removed by the cleanup below).
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
            .map_err(|e| {
                O2Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("Cannot create {}: {}", path.display(), e),
                ))
            })?;

        let result = Self::stream_response_to_file(resp, file, path).await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(path).await;
        }
        result
    }

    async fn stream_response_to_file(
        resp: reqwest::Response,
        mut file: tokio::fs::File,
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

#[derive(Debug, Deserialize)]
struct LoginData {
    access_token: String,
    jsessionid: String,
    validationkey: String,
    #[serde(rename = "encryption-token")]
    encryption_token: String,
    #[serde(default)]
    roles: Vec<Role>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Role {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct LoginResponse {
    pub access_token: String,
    pub jsessionid: String,
    pub validationkey: String,
    pub encryption_token: String,
    pub roles: Vec<Role>,
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

/// A media item returned by `POST /sapi/media?action=get`.
#[derive(Debug, Deserialize)]
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
    /// Catch-all for fields we don't use (etag, mediatype, status, date, …).
    #[serde(flatten)]
    _rest: serde_json::Value,
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
