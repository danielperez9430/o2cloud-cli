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
        "the session has expired; run the command again to log in (with \
         `upload-dir --skip-existing` it carries on where it stopped)"
    )]
    SessionExpired,

    /// A server response could not be parsed into the expected shape.
    #[error("Invalid response: {0}")]
    Parse(String),
}
