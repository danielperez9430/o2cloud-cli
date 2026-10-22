use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::error::O2Error;

/// Stored authentication tokens after a successful login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub validationkey: String,
    pub jsessionid: String,
    /// OAuth token from the old Mobile Connect login, used for silent
    /// renewal. Empty for sessions from the current OAuth web login.
    #[serde(default)]
    pub access_token: String,
    #[serde(rename = "encryption-token", default)]
    pub encryption_token: String,
    #[serde(default)]
    pub msisdn: String,
    #[serde(default)]
    pub platform: String,
    /// `x-deviceid` the session was created with.
    #[serde(default)]
    pub device_id: String,
    /// When the session was created (Unix seconds); 0 if unknown.
    #[serde(default)]
    pub logged_in_at: u64,
}

/// Current time in Unix seconds.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl AuthConfig {
    /// Build the `Authorization: oauth <base64>` header value sent by the
    /// old desktop client with SAPI calls (list, upload, download).  The
    /// format is a base64-encoded JSON object wrapping the login access
    /// token with camelCase keys.  `None` for sessions without a stored
    /// token, which authenticate with the session cookie alone.
    pub fn sapi_auth_header(&self) -> Option<String> {
        if self.access_token.is_empty() {
            return None;
        }

        #[derive(Serialize)]
        struct OAuthData {
            accesstoken: String,
            expiresin: String,
            lastrefreshdate: u64,
            msisdn: String,
            platform: String,
            keepmelogged: bool,
        }

        #[derive(Serialize)]
        struct OAuthPayload {
            data: OAuthData,
        }

        // The stored access_token is a base64-encoded JSON wrapper (JWT-style).
        // The actual token we need is inside: `data.accesstoken` (the `pat=...` value).
        let raw_token = self.extract_pat_token();

        use base64::Engine;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let payload = OAuthPayload {
            data: OAuthData {
                accesstoken: raw_token,
                expiresin: "3600".into(),
                lastrefreshdate: now,
                msisdn: self.msisdn.clone(),
                platform: self.platform.clone(),
                keepmelogged: false,
            },
        };

        let json = serde_json::to_string(&payload).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        Some(format!("oauth {}", b64))
    }

    /// Extract the raw `pat=...` token from the JWT-wrapped access_token.
    fn extract_pat_token(&self) -> String {
        use base64::Engine;
        let b64 = &self.access_token;
        let padding = 4 - b64.len() % 4;
        let padded = if padding != 4 {
            format!("{}{}", b64, "=".repeat(padding))
        } else {
            b64.clone()
        };

        if let Ok(json) = base64::engine::general_purpose::STANDARD.decode(&padded) {
            if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&json) {
                if let Some(token) = parsed
                    .get("data")
                    .and_then(|d| d.get("accesstoken"))
                    .and_then(|v| v.as_str())
                {
                    return token.to_string();
                }
            }
        }

        // Fallback: use the access_token as-is
        self.access_token.clone()
    }
}

/// Returns the configuration directory: `~/.config/o2cli/`
pub fn config_dir() -> Result<PathBuf, O2Error> {
    dirs::config_dir()
        .map(|d| d.join("o2cli"))
        .ok_or_else(|| O2Error::Config("Could not determine config directory".into()))
}

/// Returns the path to the auth file: `~/.config/o2cli/auth.json`
fn auth_file() -> Result<PathBuf, O2Error> {
    Ok(config_dir()?.join("auth.json"))
}

/// Save authentication configuration to disk.
pub fn save_auth(config: &AuthConfig) -> Result<(), O2Error> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| {
        O2Error::Config(format!(
            "Failed to create config directory {}: {}",
            dir.display(),
            e
        ))
    })?;

    let path = auth_file()?;
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| O2Error::Config(format!("Failed to serialize auth config: {}", e)))?;

    let write_err = |e: std::io::Error| {
        O2Error::Config(format!(
            "Failed to write auth file {}: {}",
            path.display(),
            e
        ))
    };

    // Restrict permissions to the owner (0700 dir, 0600 file). The file is
    // created with mode 0600 up front so the tokens are never readable by
    // other users, not even briefly between write and chmod.
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).ok();

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(write_err)?;
        // `mode` only applies on creation; tighten pre-existing files too.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(write_err)?;
        std::io::Write::write_all(&mut file, json.as_bytes()).map_err(write_err)?;
    }
    #[cfg(not(unix))]
    fs::write(&path, json).map_err(write_err)?;

    Ok(())
}

/// Load authentication configuration from disk.
pub fn load_auth() -> Result<AuthConfig, O2Error> {
    let path = auth_file()?;
    let json = fs::read_to_string(&path).map_err(|e| {
        O2Error::Config(format!(
            "Failed to read auth file {}: {}. Have you logged in?",
            path.display(),
            e
        ))
    })?;

    let config: AuthConfig = serde_json::from_str(&json)
        .map_err(|e| O2Error::Config(format!("Failed to parse auth file: {}", e)))?;

    Ok(config)
}

/// Check if authentication configuration exists on disk.
#[allow(dead_code)]
pub fn is_logged_in() -> bool {
    auth_file().is_ok_and(|p| p.exists())
}

/// Remove stored authentication configuration.
pub fn clear_auth() -> Result<(), O2Error> {
    let path = auth_file()?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| {
            O2Error::Config(format!(
                "Failed to remove auth file {}: {}",
                path.display(),
                e
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_access_token(token: &str) -> AuthConfig {
        AuthConfig {
            validationkey: "vk".into(),
            jsessionid: "js".into(),
            access_token: token.into(),
            encryption_token: "et".into(),
            msisdn: "600000000".into(),
            platform: "macos".into(),
            device_id: String::new(),
            logged_in_at: 0,
        }
    }

    #[test]
    fn test_extract_pat_token_valid_padded() {
        use base64::Engine;
        let json = r#"{"data":{"accesstoken":"pat=abc123"}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        let cfg = config_with_access_token(&b64);
        assert_eq!(cfg.extract_pat_token(), "pat=abc123");
    }

    #[test]
    fn test_extract_pat_token_valid_no_padding() {
        use base64::Engine;
        let json = r#"{"data":{"accesstoken":"pat=abc123"}}"#;
        let unpadded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(json.as_bytes());
        assert!(!unpadded.contains('=')); // sanity: genuinely no padding
        let cfg = config_with_access_token(&unpadded);
        assert_eq!(cfg.extract_pat_token(), "pat=abc123");
    }

    #[test]
    fn test_extract_pat_token_fallback_on_garbage() {
        let cfg = config_with_access_token("not-valid-base64-json!!!");
        assert_eq!(cfg.extract_pat_token(), "not-valid-base64-json!!!");
    }
}
