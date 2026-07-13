use thiserror::Error;

#[derive(Error, Debug)]
pub enum O2Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

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

    /// A server response could not be parsed into the expected shape.
    #[error("Invalid response: {0}")]
    Parse(String),
}
