use crate::api::O2Client;
use crate::config::{self, AuthConfig};
use crate::error::O2Error;
use crate::webview;

/// Run the full login flow and persist tokens to disk.
///
/// This follows the O2 Cloud web client's OAuth2 login (PKCE handled by
/// the O2 Cloud server, so no client secret is involved):
///
/// 1. Initialise a session (`JSESSIONID` cookie)
/// 2. Ask O2 Cloud to start the login → Telefónica authorization URL
/// 3. Open WebView → user enters phone number + SMS code → intercept the
///    redirect back to O2 Cloud carrying the OAuth2 code + state
/// 4. Hand the code to O2 Cloud in the same session → `JSESSIONID` and
///    `validationKey` cookies
/// 5. Save tokens to `~/.config/o2cli/auth.json`
pub async fn login() -> Result<AuthConfig, O2Error> {
    let client = O2Client::new("")?;

    // Step 1 – Initialise session (the PKCE state is bound to it)
    eprintln!("→ Initialising session...");
    client.init_session().await?;

    // Step 2 – Start the OAuth login
    eprintln!("→ Starting login...");
    let authorize_url = client.start_oauth_login().await?;

    // Step 3 – WebView OAuth2 (must run on main thread for macOS/Cocoa)
    eprintln!("→ Opening login window (enter your phone number there)...");
    let (code, state) = webview::intercept_oauth_code_sync(&authorize_url)?;
    eprintln!("✓ Authorization code received");

    // Step 4 – Complete the login
    eprintln!("→ Logging in...");
    let session = client.complete_oauth_login(&code, &state).await?;

    let mut config = AuthConfig {
        validationkey: session.validationkey,
        jsessionid: session.jsessionid,
        access_token: String::new(),
        encryption_token: String::new(),
        msisdn: String::new(),
        platform: "web".into(),
        device_id: client.device_id().to_string(),
        logged_in_at: config::now_unix(),
    };
    // Only used for display, so a failure here doesn't fail the login.
    if let Ok(Some(msisdn)) = client.get_profile_msisdn(&config).await {
        config.msisdn = msisdn;
    }

    config::save_auth(&config)?;
    eprintln!("✓ Login successful — tokens saved");

    Ok(config)
}

pub fn login_manual(phone: &str, jsessionid: &str, validationkey: &str) -> Result<AuthConfig, O2Error> {
    let config = AuthConfig {
        validationkey: validationkey.to_string(),
        jsessionid: jsessionid.to_string(),
        access_token: "".to_string(),
        encryption_token: "".to_string(),
        msisdn: phone.to_string(),
        platform: "web".to_string(),
        device_id: crate::api::generate_device_id(),
        logged_in_at: crate::config::now_unix(),
    };
    
    // Korrekter Aufruf der save_auth Funktion aus dem config-Modul
    crate::config::save_auth(&config)?;
    Ok(config)
}
