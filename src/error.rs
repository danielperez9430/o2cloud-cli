use thiserror::Error;

#[derive(Error, Debug)]
pub enum O2Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

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
}
