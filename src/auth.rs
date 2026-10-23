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

/// Log in with the `JSESSIONID` and `validationkey` cookies of a browser
/// session, for machines where the login window can't open (headless,
/// some Wayland setups). The cookies are checked against O2 Cloud before
/// they are saved.
pub async fn login_manual(jsessionid: &str, validationkey: &str) -> Result<AuthConfig, O2Error> {
    let client = O2Client::new("")?;

    let mut config = AuthConfig {
        validationkey: validationkey.trim().to_string(),
        jsessionid: jsessionid.trim().to_string(),
        access_token: String::new(),
        encryption_token: String::new(),
        msisdn: String::new(),
        platform: "web".into(),
        device_id: client.device_id().to_string(),
        logged_in_at: config::now_unix(),
    };

    eprintln!("→ Checking session...");
    if !client.is_session_valid(&config).await? {
        return Err(O2Error::Auth(
            "O2 Cloud rejected the session cookies; copy fresh JSESSIONID and \
             validationkey values from a logged-in browser"
                .into(),
        ));
    }
    // Only used for display, so a failure here doesn't fail the login.
    if let Ok(Some(msisdn)) = client.get_profile_msisdn(&config).await {
        config.msisdn = msisdn;
    }

    config::save_auth(&config)?;
    eprintln!("✓ Login successful — tokens saved");

    Ok(config)
}
