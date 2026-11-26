use thiserror::Error;

#[derive(Error, Debug)]
pub enum O2Error {
    /// Without its URL (see the `From` below).
    #[error("HTTP request failed: {0}")]
    Http(reqwest::Error),

    /// Login / session failures: bad credentials, expired session,
    /// silent re-login or OAuth exchange errors.
    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("WebView error: {0}")]
    WebView(String),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A SAPI call for an authenticated (non-login) endpoint returned a
    /// non-2xx status or an error payload — e.g. "create folder failed".
    #[error("API error: {0}")]
    Api(String),

    /// The requested folder, file, or media item does not exist.
    #[error("Not found: {0}")]
    NotFound(String),

    /// A request the CLI refuses or can't carry out as asked (bad
    /// argument, would overwrite something, not supported yet).
    #[error("{0}")]
    Invalid(String),

    /// A name matched several items; the message lists them.
    #[error("{0}")]
    Ambiguous(String),

    /// The server answered 401 in the middle of a command, or the session
    /// is over and no login window could be opened: it expired (a web
    /// login ends after a while without use) or was revoked.
    #[error(
        "Session expired: run the command again to log in, or `o2cloud login` \
         (an upload or download carries on where it stopped)"
    )]
    SessionExpired,

    /// No stored session: never logged in, or logged out.
    #[error("Not logged in: run `o2cloud login`")]
    NotLoggedIn,

    /// Something with that name is already there, and the CLI won't
    /// overwrite it or make a second copy.
    #[error("{0}")]
    Conflict(String),

    /// Some files of a run failed; the others were done.
    #[error("{0}")]
    Partial(String),

    /// A server response could not be parsed into the expected shape.
    #[error("Invalid response: {0}")]
    Parse(String),
}

/// A request error loses its URL: reqwest prints it, and it carries the
/// session's `validationkey` or a signed download link, which would end up
/// in logs people attach to bug reports.
impl From<reqwest::Error> for O2Error {
    fn from(e: reqwest::Error) -> Self {
        O2Error::Http(e.without_url())
    }
}

impl O2Error {
    /// A stable name for the kind of error, for `--json` and scripts.
    pub fn code(&self) -> &'static str {
        match self {
            O2Error::Http(_) => "network",
            O2Error::Auth(_) => "auth_failed",
            O2Error::Config(_) => "config",
            O2Error::WebView(_) => "login_window",
            O2Error::UrlParse(_) => "invalid",
            O2Error::Io(_) => "io",
            O2Error::Api(_) if self.server_side() => "server_error",
            O2Error::Api(_) => "api_error",
            O2Error::NotFound(_) => "not_found",
            O2Error::Invalid(_) => "invalid",
            O2Error::Ambiguous(_) => "ambiguous",
            O2Error::SessionExpired => "session_expired",
            O2Error::NotLoggedIn => "not_logged_in",
            O2Error::Conflict(_) => "conflict",
            O2Error::Partial(_) => "partial_failure",
            O2Error::Parse(_) => "bad_response",
        }
    }

    /// The exit status for this error: 2 a request the CLI won't carry
    /// out as given (as for a usage error), 3 not found, 4 not logged in
    /// or session expired, 5 a conflict with something already there,
    /// 6 some files of a run failed, 7 the network or the server failed
    /// (worth trying again), 1 anything else.
    pub fn exit_code(&self) -> i32 {
        match self {
            O2Error::Invalid(_) | O2Error::Ambiguous(_) | O2Error::UrlParse(_) => 2,
            O2Error::NotFound(_) => 3,
            O2Error::SessionExpired | O2Error::NotLoggedIn | O2Error::Auth(_) => 4,
            O2Error::Conflict(_) => 5,
            O2Error::Partial(_) => 6,
            O2Error::Http(_) | O2Error::Parse(_) => 7,
            O2Error::Api(_) if self.server_side() => 7,
            _ => 1,
        }
    }

    /// Whether running the same command again may well work: the network
    /// or the server failed, or some files of a run did (it carries on).
    pub fn retryable(&self) -> bool {
        matches!(self.exit_code(), 6 | 7)
    }

    /// An API error the server caused (5xx, 429, or an outage answer).
    fn server_side(&self) -> bool {
        let O2Error::Api(msg) = self else {
            return false;
        };
        msg.contains("(status 5") || msg.contains("(status 429") || msg.contains("FOL-1000")
    }

    /// The error as JSON, for `--json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "code": self.code(),
                "message": crate::redact::text(&self.to_string()),
                "retryable": self.retryable(),
                "exit_code": self.exit_code(),
                "run": crate::log::run_id(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codes() {
        let e = O2Error::SessionExpired;
        assert_eq!(
            (e.code(), e.exit_code(), e.retryable()),
            ("session_expired", 4, false)
        );
        // The Tauri app and scripts match these words.
        assert!(e.to_string().starts_with("Session expired"));
        assert!(O2Error::NotLoggedIn
            .to_string()
            .starts_with("Not logged in"));
        let e = O2Error::Api("Upload failed (status 502 Bad Gateway): x".into());
        assert_eq!(
            (e.code(), e.exit_code(), e.retryable()),
            ("server_error", 7, true)
        );
        let e = O2Error::Api("Rename failed: MED-1005".into());
        assert_eq!(
            (e.code(), e.exit_code(), e.retryable()),
            ("api_error", 1, false)
        );
        assert_eq!(O2Error::Conflict("x".into()).exit_code(), 5);
        assert_eq!(O2Error::Partial("x".into()).exit_code(), 6);
        assert_eq!(O2Error::NotFound("x".into()).exit_code(), 3);
        assert_eq!(O2Error::Invalid("x".into()).exit_code(), 2);
        let json = O2Error::Conflict("a.txt is there".into()).to_json();
        assert_eq!(json["error"]["retryable"], false);
        assert_eq!(json["error"]["code"], "conflict");
        assert_eq!(json["error"]["message"], "a.txt is there");
        assert_eq!(json["error"]["exit_code"], 5);
    }

    /// A session's keys never show in an error, whatever its text.
    #[test]
    fn test_no_session_keys_in_errors() {
        let auth = crate::config::AuthConfig {
            validationkey: "CANARY-VK-0001".into(),
            jsessionid: "CANARY-JS-0002".into(),
            access_token: "CANARY-AT-0003".into(),
            encryption_token: "CANARY-ET-0004".into(),
            msisdn: "34600111222".into(),
            platform: "ios".into(),
            device_id: "CANARY-DEV-0005".into(),
            logged_in_at: 0,
            oauth_client: Some(crate::config::OAuthClient {
                id: "CANARY-OID-0006".into(),
                secret: "CANARY-OSEC-0007".into(),
            }),
        };
        crate::redact::register_auth(&auth);
        let all = "CANARY-VK-0001 CANARY-JS-0002 CANARY-AT-0003 CANARY-ET-0004 34600111222 \
                   CANARY-DEV-0005 CANARY-OID-0006 CANARY-OSEC-0007";
        for e in [
            O2Error::Auth(format!("login failed: {all}")),
            O2Error::Api(format!("Upload failed (status 500): {all}")),
            O2Error::Parse(format!("bad body {all}")),
            O2Error::Invalid(format!("x {all}")),
        ] {
            let shown = [crate::redact::text(&e.to_string()), e.to_json().to_string()];
            for text in shown {
                assert!(
                    !text.contains("CANARY") && !text.contains("34600111222"),
                    "{text}"
                );
            }
        }
    }

    /// No session key or signed link in a request error's message.
    #[tokio::test]
    async fn test_http_error_without_url() {
        // Nothing listens on port 9 (discard) of localhost.
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:9/sapi/media?action=get&validationkey=SECRETKEY123")
            .send()
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("SECRETKEY123"),
            "reqwest prints it"
        );
        let e = O2Error::from(err);
        for text in [e.to_string(), e.to_json().to_string()] {
            assert!(!text.contains("SECRETKEY123"), "{text}");
            assert!(!text.contains("127.0.0.1:9"), "{text}");
        }
        assert!(e.to_string().starts_with("HTTP request failed"));
    }
}
