use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::error::O2Error;

/// Stored authentication tokens after a successful login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub validationkey: String,
    pub jsessionid: String,
    pub access_token: String,
    #[serde(rename = "encryption-token")]
    pub encryption_token: String,
    pub msisdn: String,
    pub platform: String,
}

impl AuthConfig {
    /// Build the `Authorization: oauth <base64>` header value required
    /// by SAPI calls (list, upload, download).  The format is a
    /// base64-encoded JSON object wrapping the login access token with
    /// camelCase keys, matching what the official desktop client sends.
    pub fn to_sapi_auth_header(&self) -> String {
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
        format!("oauth {}", b64)
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
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("Could not determine config directory")
        .join("o2cli")
}

/// Returns the path to the auth file: `~/.config/o2cli/auth.json`
fn auth_file() -> PathBuf {
    config_dir().join("auth.json")
}

/// Save authentication configuration to disk.
pub fn save_auth(config: &AuthConfig) -> Result<(), O2Error> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| {
        O2Error::Config(format!(
            "Failed to create config directory {}: {}",
            dir.display(),
            e
        ))
    })?;

    let path = auth_file();
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| O2Error::Config(format!("Failed to serialize auth config: {}", e)))?;

    fs::write(&path, json).map_err(|e| {
        O2Error::Config(format!(
            "Failed to write auth file {}: {}",
            path.display(),
            e
        ))
    })?;

    // Set restrictive permissions on the auth file (owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).ok();
    }

    Ok(())
}

/// Load authentication configuration from disk.
pub fn load_auth() -> Result<AuthConfig, O2Error> {
    let path = auth_file();
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
    auth_file().exists()
}

/// Remove stored authentication configuration.
pub fn clear_auth() -> Result<(), O2Error> {
    let path = auth_file();
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
