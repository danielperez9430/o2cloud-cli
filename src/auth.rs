use crate::api::O2Client;
use crate::config::{self, AuthConfig};
use crate::error::O2Error;
use crate::webview;

/// Run the full login flow and persist tokens to disk.
///
/// 1. Start Mobile Connect → get Telefónica authorization URL
/// 2. Open WebView → user authenticates → intercept OAuth2 code + state
/// 3. Validate credential → exchange code for credential token
/// 4. Login → obtain session tokens (validationkey, jsessionid, …)
/// 5. Save tokens to `~/.config/o2cli/auth.json`
pub async fn login(phone_number: &str) -> Result<AuthConfig, O2Error> {
    let client = O2Client::new()?;

    // Step 0 – Initialise session (get JSESSIONID cookie)
    eprintln!("→ Initialising session...");
    client.init_session().await?; // ignore returned JSESSIONID — login sets a new one

    // Step 1 – Start Mobile Connect
    eprintln!("→ Starting Mobile Connect for +34 {}...", phone_number);
    let start = client.start_mobile_connect(phone_number).await?;

    // Step 2 – WebView OAuth2 (must run on main thread for macOS/Cocoa)
    eprintln!("→ Opening login window...");
    let (code, state) = webview::intercept_oauth_code_sync(&start.authorizationurl)?;
    eprintln!("✓ Authorization code received");

    // Step 3 – Validate credential
    eprintln!("→ Validating credential...");
    let credential = client.validate_credential(&code, &state).await?;

    // Step 4 – Login (requires OAuth Authorization header from validate response)
    eprintln!("→ Logging in...");
    let oauth_header = credential.to_oauth_auth_header();
    let login_resp = client.login(&oauth_header).await?;

    // Build and save config
    let config = AuthConfig {
        validationkey: login_resp.validationkey,
        jsessionid: login_resp.jsessionid,
        access_token: login_resp.access_token,
        encryption_token: login_resp.encryption_token,
        msisdn: credential.msisdn,
        platform: credential.platform,
    };

    config::save_auth(&config)?;
    eprintln!("✓ Login successful — tokens saved");

    Ok(config)
}
