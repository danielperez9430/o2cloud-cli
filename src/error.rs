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

    /// The server answered 401 in the middle of a command: the session
    /// expired (a web login ends after a while without use) or was revoked.
    #[error(
        "Session expired: run the command again to log in (with \
         `upload-dir --skip-existing` it carries on where it stopped)"
    )]
    SessionExpired,

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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!e.to_string().contains("SECRETKEY123"), "{e}");
        assert!(e.to_string().starts_with("HTTP request failed"));
    }

    #[test]
    fn test_session_expired_wording() {
        // Apps match these words (the Tauri app looks for "Session expired").
        assert!(O2Error::SessionExpired
            .to_string()
            .starts_with("Session expired"));
    }
}
