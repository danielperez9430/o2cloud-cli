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
            .header(header::ORIGIN, WEB_ORIGIN)
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
            .header(header::ORIGIN, WEB_ORIGIN)
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
            .send()
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
        let resp = builder.send().await?;

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
                    fields: vec!["name", "size", "etag", "folderid", "url", "creationdate"],
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
        if let Some(err) = sapi_error(&raw_body) {
            return Err(O2Error::Api(format!("Create folder failed: {}", err)));
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
        let resp = self.with_sapi_headers(req, auth).send().await?;
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
        let url = format!("{}{}", BASE_URL, path);
        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[("action", action), ("validationkey", &auth.validationkey)])
            .json(body);
        let resp = self.with_sapi_headers(req, auth).send().await?;
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
            return Err(O2Error::Api(format!("{} failed: {}", what, err)));
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
    /// `{"folderid": ...}` to move it.
    ///
    /// `POST /sapi/upload/file?action=save-metadata`, form-encoded
    /// `data={"data":{"id":<id>, ...changes}}`
    pub async fn update_file(
        &self,
        media_id: u64,
        changes: serde_json::Value,
        auth: &AuthConfig,
    ) -> Result<(), O2Error> {
        let mut data = serde_json::json!({ "id": media_id });
        if let (Some(obj), Some(changes)) = (data.as_object_mut(), changes.as_object()) {
            obj.extend(changes.clone());
        }
        let form_value = serde_json::json!({ "data": data }).to_string();

        let url = format!("{}/sapi/upload/file", BASE_URL);
        let req = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .query(&[
                ("action", "save-metadata"),
                ("validationkey", &auth.validationkey),
            ])
            .form(&[("data", form_value)]);
        let resp = self.with_sapi_headers(req, auth).send().await?;
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
            return Err(O2Error::Api(format!("Update file failed: {}", err)));
        }
        Ok(())
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
#[derive(Debug, Deserialize)]
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
    /// When the file was uploaded, in Unix milliseconds. Sent even when
    /// not asked for (asking for it is rejected).
    #[serde(default)]
    pub date: Option<u64>,
    /// When the file itself was created (for a photo, when it was taken),
    /// in Unix milliseconds.
    #[serde(default)]
    pub creationdate: Option<u64>,
    /// Catch-all for fields we don't use (etag, mediatype, status, …).
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
