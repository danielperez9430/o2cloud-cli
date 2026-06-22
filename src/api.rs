use reqwest::{header, Client};
use serde::{Deserialize, Serialize};

use crate::config::AuthConfig;
use crate::error::O2Error;

/// Base URL for the O2 Cloud SAPI (Synchronoss API).
const BASE_URL: &str = "https://cloud.o2online.es";
/// Separate host for binary uploads.
const UPLOAD_URL: &str = "https://upload.cloud.o2online.es";

/// Generate a random device ID in the format used by the web client.
fn generate_device_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!(
        "web-{:08x}{}{}",
        now as u32,
        (now >> 32) as u32,
        (now as u32).wrapping_mul(0x9e3779b9)
    )
}

/// O2 Cloud API client.
///
/// Holds a `reqwest::Client` with cookie storage enabled so that
/// JSESSIONID cookies are preserved across requests.  Headers are
/// set to mimic the official O2 Cloud desktop app (QtWebEngine).
pub struct O2Client {
    client: Client,
    device_id: String,
}

impl O2Client {
    /// Create a new API client with browser-matching defaults.
    pub fn new() -> Result<Self, O2Error> {
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

        let device_id = generate_device_id();

        let client = Client::builder()
            .cookie_store(true)
            .default_headers(headers)
            .build()?;

        Ok(Self { client, device_id })
    }

    /// Initialise the session by fetching the login page.
    ///
    /// This is required to obtain a `JSESSIONID` cookie from the server
    /// before making any SAPI calls — CloudFront blocks requests that
    /// lack a valid session cookie.
    pub async fn init_session(&self) -> Result<Option<String>, O2Error> {
        let url = format!("{}/ui/html/mobileconnect.html", BASE_URL);

        let resp = self
            .client
            .get(&url)
            .query(&[("embedded", "true")])
            .header("x-deviceid", &self.device_id)
            .header("referer", "https://cloud.o2online.es/")
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty")
            .send()
            .await?;

        // Extract the new JSESSIONID from Set-Cookie
        let new_jsessionid = Self::extract_jsessionid_from_headers(resp.headers());

        // Drain the response body so the connection is reusable.
        let _ = resp.text().await?;
        Ok(new_jsessionid)
    }

    /// Check if the stored session is still valid by probing an
    /// authenticated endpoint.  Returns the auth header so callers
    /// can reuse it.
    async fn session_probe(&self, auth: &AuthConfig) -> Result<(), O2Error> {
        let url = format!("{}/sapi/profile/role", BASE_URL);
        let auth_header = auth.to_sapi_auth_header();
        let resp = self
            .client
            .get(&url)
            .query(&[("action", "get"), ("validationkey", &auth.validationkey)])
            .header("cookie", format!("JSESSIONID={}", auth.jsessionid))
            .header("authorization", &auth_header)
            .send()
            .await?;
        let status = resp.status();
        let _ = resp.text().await?;
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 401 {
            Err(O2Error::Auth("session expired".into()))
        } else {
            Ok(()) // non-401 = probably transient, don't block
        }
    }

    /// Probe the session.  Returns `true` if the session is valid.
    pub async fn is_session_valid(&self, auth: &AuthConfig) -> bool {
        self.session_probe(auth).await.is_ok()
    }

    /// Attempt a silent re-login using the stored `access_token` (pat token).
    /// First gets a fresh JSESSIONID cookie, then calls the login endpoint.
    /// If successful, returns new `LoginResponse` with fresh tokens.
    pub async fn silent_login(&self, auth: &AuthConfig) -> Result<LoginResponse, O2Error> {
        // Get a fresh session cookie first
        self.init_session().await?;

        let url = format!("{}/sapi/login", BASE_URL);
        let auth_header = auth.to_sapi_auth_header();

        // Try with SAPI auth header (pat token) — this is what the
        // desktop app uses for all its API calls
        let resp = self
            .client
            .post(&url)
            .query(&[("action", "login"), ("responsetime", "true")])
            .header("authorization", &auth_header)
            .header("content-type", "application/x-www-form-urlencoded; charset=UTF-8")
            .header("x-deviceid", &self.device_id)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Silent login failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        let body: RawResponse<LoginData> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Auth(format!(
                "Failed to parse silent login response: {} — body: {}",
                e,
                &raw_body[..raw_body.len().min(300)]
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
    // Session
    // ------------------------------------------------------------------

    /// Grab the JSESSIONID value from a `Set-Cookie` response header.
    fn extract_jsessionid_from_headers(
        headers: &reqwest::header::HeaderMap,
    ) -> Option<String> {
        headers.get_all("set-cookie").iter().find_map(|v| {
            let s = v.to_str().ok()?;
            if s.starts_with("JSESSIONID=") {
                s.split(';').next().and_then(|c| {
                    c.strip_prefix("JSESSIONID=").map(|v| v.to_string())
                })
            } else {
                None
            }
        })
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Attach per-request headers that CloudFront / WAF expects from a
    /// browser client: device ID, referer, and Sec-Fetch metadata.
    fn with_device_headers(
        &self,
        builder: reqwest::RequestBuilder,
        path: &str,
    ) -> reqwest::RequestBuilder {
        builder
            .header("x-deviceid", &self.device_id)
            .header("referer", format!("https://cloud.o2online.es{}", path))
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "empty")
    }

    /// Attach headers for SAPI calls (list, upload, download).  These
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
        builder
            .header("x-deviceid", &self.device_id)
            .header("x-request-id", req_id)
            .header("cookie", format!("JSESSIONID={}", auth.jsessionid))
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
        let req = self
            .client
            .get(&url)
            .query(&[("action", "get")]);

        let resp = self
            .with_device_headers(req, "/sapi/system/information")
            .send()
            .await?;

        let body: RawResponse<SystemInfo> = resp.json().await?;
        Ok(body.data)
    }

    // ------------------------------------------------------------------
    // Mobile Connect – Step 1: Start
    // ------------------------------------------------------------------

    /// Start the Mobile Connect authentication flow.
    ///
    /// `POST /sapi/login/mobileconnect?action=start`
    ///
    /// The body is URL-encoded form data: `platform=<platform>&msisdn=<phone>`.
    /// Returns the Telefónica Mobile Connect authorization URL.
    pub async fn start_mobile_connect(
        &self,
        msisdn: &str,
    ) -> Result<StartMobileConnectResponse, O2Error> {
        let url = format!("{}/sapi/login/mobileconnect", BASE_URL);

        // The API expects the full MSISDN with country code prefix
        let full_msisdn = if msisdn.starts_with("34") {
            msisdn.to_string()
        } else {
            format!("34{}", msisdn)
        };

        let form = [("platform", "macos"), ("msisdn", &full_msisdn)];

        let req = self
            .client
            .post(&url)
            .query(&[("action", "start")])
            .header("content-type", "application/x-www-form-urlencoded; charset=UTF-8")
            .form(&form);

        let resp = self
            .with_device_headers(req, "/ui/html/mobileconnect.html")
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Start Mobile Connect failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        let body: RawResponse<StartMobileConnectData> =
            serde_json::from_str(&raw_body).map_err(|e| {
                O2Error::Auth(format!(
                    "Failed to parse start response: {} — body: {}",
                    e,
                    &raw_body[..raw_body.len().min(300)]
                ))
            })?;

        Ok(StartMobileConnectResponse {
            authorizationurl: body.data.authorizationurl,
        })
    }

    // ------------------------------------------------------------------
    // Mobile Connect – Step 2: Validate
    // ------------------------------------------------------------------

    /// Validate the OAuth2 authorization code and state.
    ///
    /// `POST /sapi/credential/mobileconnect?action=validate`
    ///
    /// Body is JSON: `{"data":{"code":"...","state":"..."}}`
    pub async fn validate_credential(
        &self,
        code: &str,
        state: &str,
    ) -> Result<ValidateCredentialResponse, O2Error> {
        let url = format!("{}/sapi/credential/mobileconnect", BASE_URL);

        let payload = ValidateCredentialRequest {
            data: ValidateCredentialRequestData {
                code: code.to_string(),
                state: state.to_string(),
            },
        };

        let req = self
            .client
            .post(&url)
            .query(&[("action", "validate")])
            .json(&payload);

        let resp = self
            .with_device_headers(req, "/ui/html/clientoauth.html")
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Validate credential failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        let body: RawResponse<ValidateCredentialData> =
            serde_json::from_str(&raw_body).map_err(|e| {
                O2Error::Auth(format!(
                    "Failed to parse validate response: {} — body: {}",
                    e,
                    &raw_body[..raw_body.len().min(300)]
                ))
            })?;

        Ok(ValidateCredentialResponse {
            access_token: body.data.access_token,
            expires_in: body.data.expires_in,
            msisdn: body.data.msisdn,
            platform: body.data.platform,
            lastrefreshdate: body.data.lastrefreshdate,
        })
    }

    // ------------------------------------------------------------------
    // Login – Final Step
    // ------------------------------------------------------------------

    /// Complete login and obtain session tokens.
    ///
    /// `POST /sapi/login?action=login&responsetime=true`
    ///
    /// The body is empty.  Returns the `validationkey`, `jsessionid`,
    /// JWT `access_token`, and `encryption-token` needed for all
    /// subsequent API calls.
    pub async fn login(&self, oauth_auth_header: &str) -> Result<LoginResponse, O2Error> {
        let url = format!("{}/sapi/login", BASE_URL);

        let req = self
            .client
            .post(&url)
            .query(&[("action", "login"), ("responsetime", "true")])
            .header("authorization", oauth_auth_header);

        let resp = self
            .with_device_headers(req, "/ui/html/clientoauth.html")
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Login failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        let body: RawResponse<LoginData> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Auth(format!(
                "Failed to parse login response: {} — body: {}",
                e,
                &raw_body[..raw_body.len().min(300)]
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
    // Folder / Media Listing
    // ------------------------------------------------------------------

    /// Get ALL folders in the account.  Returns a flat list; callers
    /// should build the tree by resolving `parentid` links.
    ///
    /// `POST /sapi/media/folder?action=get`
    pub async fn get_all_folders(&self, auth: &AuthConfig) -> Result<Vec<FolderEntry>, O2Error> {
        let url = format!("{}/sapi/media/folder", BASE_URL);
        let auth_header = auth.to_sapi_auth_header();

        let req = self
            .client
            .post(&url)
            .query(&[("action", "get"), ("validationkey", &auth.validationkey)])
            .header("authorization", &auth_header);

        let resp = self
            .with_sapi_headers(req, auth)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Get folders failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        #[derive(Deserialize)]
        struct FolderData {
            folders: Vec<FolderEntry>,
        }

        let body: RawResponse<FolderData> = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Auth(format!("Failed to parse folder list: {} — body: {}", e, &raw_body[..raw_body.len().min(300)]))
        })?;

        Ok(body.data.folders)
    }

    /// Get all media items with requested fields.  Pages through results
    /// automatically (1000 items per request).
    ///
    /// `POST /sapi/media?action=get` with JSON body specifying fields.
    pub async fn get_all_media(&self, auth: &AuthConfig) -> Result<Vec<MediaItem>, O2Error> {
        let url = format!("{}/sapi/media", BASE_URL);
        let auth_header = auth.to_sapi_auth_header();
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
                .query(&[
                    ("action", "get"),
                    ("validationkey", &auth.validationkey),
                    ("limit", &page_size.to_string()),
                    ("offset", &offset.to_string()),
                ])
                .header("authorization", &auth_header)
                .header("content-type", "application/json")
                .json(&payload);

            let resp = self
                .with_sapi_headers(req, auth)
                .send()
                .await?;

            let status = resp.status();
            let raw_body = resp.text().await?;

            if !status.is_success() {
                return Err(O2Error::Auth(format!(
                    "Get media failed (status {}): {}",
                    status,
                    &raw_body[..raw_body.len().min(300)]
                )));
            }

            let body: RawResponse<MediaData> = serde_json::from_str(&raw_body).map_err(|e| {
                O2Error::Auth(format!("Failed to parse media list: {} — body: {}", e, &raw_body[..raw_body.len().min(300)]))
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
        let now = format_compact_iso_now();

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
            .query(&[("action", "save"), ("validationkey", &auth.validationkey)])
            .json(&payload);

        let resp = self
            .with_sapi_headers(req, auth)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Create folder failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        #[derive(Deserialize)]
        struct FolderCreateResponse {
            id: u64,
        }

        let body: FolderCreateResponse = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Auth(format!("Failed to parse create folder response: {} — body: {}", e, &raw_body[..raw_body.len().min(200)]))
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
        let req = self
            .client
            .post(&url)
            .query(&[
                ("action", "softdelete"),
                ("id", &folder_id.to_string()),
                ("validationkey", &auth.validationkey),
            ]);
        let resp = self.with_sapi_headers(req, auth).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(O2Error::Auth(format!(
                "Delete folder failed ({}): {}", status, &body[..body.len().min(200)]
            )));
        }
        Ok(())
    }

    /// Soft-delete a media item (move to trash).
    ///
    /// `POST /sapi/media?action=softdelete&id=<id>`
    pub async fn delete_media(&self, media_id: u64, auth: &AuthConfig) -> Result<(), O2Error> {
        let url = format!("{}/sapi/media", BASE_URL);

        let req = self
            .client
            .post(&url)
            .query(&[
                ("action", "softdelete"),
                ("id", &media_id.to_string()),
                ("validationkey", &auth.validationkey),
            ]);

        let resp = self
            .with_sapi_headers(req, auth)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Delete failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Upload
    // ------------------------------------------------------------------

    /// Save file metadata and obtain a media ID for the subsequent byte upload.
    pub async fn upload_metadata(
        &self,
        meta: &UploadMeta,
        auth: &AuthConfig,
    ) -> Result<UploadMetaResponse, O2Error> {
        let url = format!("{}/sapi/upload/file", BASE_URL);
        let auth_header = auth.to_sapi_auth_header();

        let payload = RawRequest {
            data: meta,
        };
        let json_body = serde_json::to_vec(&payload).map_err(|e| {
            O2Error::Auth(format!("Failed to serialize upload metadata: {}", e))
        })?;

        let req = self
            .client
            .post(&url)
            .query(&[
                ("action", "save-metadata"),
                ("responsetime", "true"),
                ("lastupdate", "true"),
                ("validationkey", &auth.validationkey),
            ])
            .header("content-type", "application/octet-stream")
            .header("authorization", &auth_header)
            .body(json_body);

        let resp = self
            .with_sapi_headers(req, auth)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Upload metadata failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        // Check for error-in-200 response
        if let Ok(error_body) = serde_json::from_str::<serde_json::Value>(&raw_body) {
            if error_body.get("error").is_some() {
                let msg = error_body["error"]["message"].as_str().unwrap_or("unknown");
                let params: Vec<&str> = error_body["error"]["parameters"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| p["param"].as_str()).collect())
                    .unwrap_or_default();
                return Err(O2Error::Auth(format!(
                    "Upload metadata rejected: {} (params: {:?})",
                    msg, params
                )));
            }
        }

        let resp_data: UploadMetaResponse = serde_json::from_str(&raw_body).map_err(|e| {
            O2Error::Auth(format!(
                "Failed to parse upload metadata response: {} — body: {}",
                e,
                &raw_body[..raw_body.len().min(300)]
            ))
        })?;

        Ok(resp_data)
    }

    /// Upload the raw file bytes for a previously-saved metadata entry.
    pub async fn upload_bytes(
        &self,
        media_id: &str,
        data: &[u8],
        auth: &AuthConfig,
    ) -> Result<(), O2Error> {
        let url = format!("{}/sapi/upload/file", UPLOAD_URL);
        let auth_header = auth.to_sapi_auth_header();

        let file_size = data.len().to_string();

        let req = self
            .client
            .post(&url)
            .query(&[
                ("action", "save"),
                ("id", media_id),
                ("lastupdate", "true"),
                ("acceptasynchronous", "true"),
                ("validationkey", &auth.validationkey),
            ])
            .header("content-type", "application/octet-stream")
            .header("authorization", &auth_header)
            .header("x-funambol-file-size", &file_size)
            .header("x-funambol-id", media_id)
            .body(data.to_vec());

        let resp = self
            .with_sapi_headers(req, auth)
            .send()
            .await?;

        let status = resp.status();
        let raw_body = resp.text().await?;

        if !status.is_success() {
            return Err(O2Error::Auth(format!(
                "Upload bytes failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(300)]
            )));
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Download
    // ------------------------------------------------------------------

    /// Download a file using its signed URL (from the `url` field in the
    /// media listing response).  Returns the raw bytes.
    pub async fn download_file_url(&self, download_url: &str) -> Result<bytes::Bytes, O2Error> {
        let resp = self
            .client
            .get(download_url)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let raw_body = resp.text().await.unwrap_or_default();
            return Err(O2Error::Auth(format!(
                "Download failed (status {}): {}",
                status,
                &raw_body[..raw_body.len().min(200)]
            )));
        }

        let body = resp.bytes().await?;
        Ok(body)
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

// -- Start Mobile Connect ----------------------------------------------

#[derive(Debug, Deserialize)]
struct StartMobileConnectData {
    authorizationurl: String,
}

#[derive(Debug)]
pub struct StartMobileConnectResponse {
    pub authorizationurl: String,
}

// -- Validate Credential -----------------------------------------------

#[derive(Debug, Serialize)]
struct ValidateCredentialRequest {
    data: ValidateCredentialRequestData,
}

#[derive(Debug, Serialize)]
struct ValidateCredentialRequestData {
    code: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct ValidateCredentialData {
    access_token: String,
    expires_in: String,
    msisdn: String,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    lastrefreshdate: u64,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ValidateCredentialResponse {
    pub access_token: String,
    pub expires_in: String,
    pub msisdn: String,
    pub platform: String,
    pub lastrefreshdate: u64,
}

impl ValidateCredentialResponse {
    /// Build the `Authorization: oauth <base64>` header value required
    /// by the login endpoint.  The format is a base64-encoded JSON
    /// object that wraps the validate response data with camelCase keys.
    pub fn to_oauth_auth_header(&self) -> String {
        #[derive(Serialize)]
        struct OAuthData {
            accesstoken: String,
            expiresin: String,
            lastrefreshdate: u64,
            msisdn: String,
            platform: String,
            refreshtoken: String,
        }

        #[derive(Serialize)]
        struct OAuthPayload {
            data: OAuthData,
        }

        use base64::Engine;
        let payload = OAuthPayload {
            data: OAuthData {
                accesstoken: self.access_token.clone(),
                expiresin: self.expires_in.clone(),
                lastrefreshdate: self.lastrefreshdate,
                msisdn: self.msisdn.clone(),
                platform: self.platform.clone(),
                refreshtoken: String::new(),
            },
        };

        let json = serde_json::to_string(&payload).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        format!("oauth {}", b64)
    }
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
    #[serde(default, alias = "folder", deserialize_with = "deserialize_optional_id")]
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

#[derive(Debug, Deserialize)]
pub struct UploadMetaResponse {
    pub id: String,
    #[serde(flatten)]
    _rest: serde_json::Value,
}

fn format_compact_iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = (secs / 86400) as i64;
    let (y, m, d) = {
        let mut y = 1970i64;
        let mut remaining = days;
        loop {
            let yd = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
            if remaining < yd { break; }
            remaining -= yd;
            y += 1;
        }
        let feb = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 };
        let md = [31,feb,31,30,31,30,31,31,30,31,30,31];
        let mut m = 1;
        let mut rd = remaining;
        for &mc in &md {
            if rd < mc { break; }
            rd -= mc;
            m += 1;
        }
        (y, m, rd + 1)
    };
    let t = secs % 86400;
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}", y, m, d, t / 3600, (t % 3600) / 60, t % 60)
}

