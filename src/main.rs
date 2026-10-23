mod api;
mod auth;
mod config;
mod error;
mod ops;
mod util;
mod webview;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use error::O2Error;

/// O2 Cloud CLI — access your O2 Cloud storage from the terminal.
///
/// O2 Cloud is the cloud storage service provided by Telefónica España,
/// powered by Synchronoss OneMediaHub.
#[derive(Parser)]
#[command(name = "o2cloud", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Log in to O2 Cloud (opens a login window)
    Login {
        /// No longer used: the phone number is entered in the login window.
        /// Kept so existing scripts don't break.
        #[arg(short, long, hide = true)]
        phone: Option<String>,

        /// Log in without the login window, using the JSESSIONID cookie of
        /// a logged-in browser session (needs --validationkey too)
        #[arg(
            long,
            env = "O2CLOUD_JSESSIONID",
            hide_env_values = true,
            requires = "validationkey"
        )]
        jsessionid: Option<String>,

        /// The validationkey cookie of the same browser session (needs
        /// --jsessionid too)
        #[arg(
            long,
            env = "O2CLOUD_VALIDATIONKEY",
            hide_env_values = true,
            requires = "jsessionid"
        )]
        validationkey: Option<String>,
    },

    /// Show current login status
    Status,

    /// Log out (clear stored tokens)
    Logout,

    /// List files (default: root folder contents)
    Ls {
        /// Folder path or ID (e.g. /Photos/2024 or 12345678)
        path: Option<String>,

        /// Show all files across all folders
        #[arg(short = 'a', long)]
        all: bool,

        /// Tree view (hierarchical)
        #[arg(short = 't', long)]
        tree: bool,
    },

    /// Upload a file to O2 Cloud
    Upload {
        /// Local file path
        path: String,

        /// Target folder ID (omit for root)
        #[arg(short, long)]
        folder: Option<u64>,
    },

    /// Upload a directory recursively
    UploadDir {
        /// Local directory path
        path: String,

        /// Target folder ID (omit for root)
        #[arg(short, long)]
        folder: Option<u64>,
    },

    /// Zip a directory and upload it as a single file
    UploadZip {
        /// Local directory path
        path: String,

        /// Target folder ID (omit for root)
        #[arg(short, long)]
        folder: Option<u64>,
    },

    /// Delete a file or folder (move to trash)
    Rm {
        /// File/folder ID or path (e.g. 1234567890 or /Photos)
        target: String,

        /// Recursive: delete folder and all its contents
        #[arg(short, long)]
        recursive: bool,

        /// Skip the confirmation prompt for recursive deletes
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Search files and folders by name
    Find {
        /// Search query (case-insensitive substring match)
        query: String,
    },

    /// Create a public link to a file or folder and print its URL
    Share {
        /// File/folder ID, path (e.g. /docs or /docs/a.txt), or unique file name
        target: String,
    },

    /// List shared folders (public links)
    Shares,

    /// Remove the public link of a folder
    Unshare {
        /// Folder ID or path, or a link ID from `o2cloud shares`
        target: String,
    },

    /// Move a file into another folder
    Mv {
        /// File ID, path (e.g. /docs/a.txt), or unique file name
        source: String,

        /// Destination folder ID or path (e.g. /archive)
        destination: String,
    },

    /// Rename a file
    Rename {
        /// File ID, path (e.g. /docs/a.txt), or unique file name
        target: String,

        /// New name (just the name, not a path)
        new_name: String,
    },

    /// Download a file from O2 Cloud
    Download {
        /// Media ID (shown in `o2cloud ls`), file path (e.g. /docs/a.txt),
        /// or file name if it is unique
        target: String,

        /// Output file path (defaults to current directory)
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Load stored credentials and an API client. If the session has
/// expired, sessions from the old login are renewed silently with their
/// stored token; otherwise (or if that fails) the login window opens.
async fn require_client_and_auth() -> Result<(api::O2Client, config::AuthConfig), O2Error> {
    let auth = config::load_auth()?;
    let client = api::O2Client::new(&auth.device_id)?;

    if client.is_session_valid(&auth).await? {
        return Ok((client, auth));
    }

    if auth.sapi_auth_header().is_some() {
        eprintln!("→ Session expired, trying silent re-login...");
        match client.silent_login(&auth).await {
            Ok(login_resp) => {
                // Update stored config with new tokens
                let new_auth = config::AuthConfig {
                    validationkey: login_resp.validationkey,
                    jsessionid: login_resp.jsessionid,
                    access_token: login_resp.access_token,
                    encryption_token: login_resp.encryption_token,
                    msisdn: auth.msisdn.clone(),
                    platform: auth.platform.clone(),
                    device_id: client.device_id().to_string(),
                    logged_in_at: config::now_unix(),
                };
                if let Err(e) = config::save_auth(&new_auth) {
                    eprintln!("Failed to save refreshed auth: {}", e);
                }
                eprintln!("✓ Session refreshed silently");
                return Ok((client, new_auth));
            }
            Err(e) => eprintln!("→ Silent re-login failed ({e})"),
        }
    } else {
        eprintln!("→ Session expired");
    }

    eprintln!(
        "→ Opening login window... (without one, run `o2cloud login --jsessionid ... \
         --validationkey ...` instead)"
    );
    let new_auth = crate::auth::login().await?;
    eprintln!("✓ Re-login successful");
    let new_client = api::O2Client::new(&new_auth.device_id)?;
    Ok((new_client, new_auth))
}

async fn run(cli: Cli) -> Result<(), O2Error> {
    match cli.command {
        Commands::Login {
            phone,
            jsessionid,
            validationkey,
        } => {
            if phone.is_some() {
                eprintln!(
                    "Note: --phone is no longer needed; enter the number in the login window."
                );
            }
            let config = match (jsessionid, validationkey) {
                (Some(jsessionid), Some(validationkey)) => {
                    auth::login_manual(&jsessionid, &validationkey).await?
                }
                _ => auth::login().await?,
            };
            if config.msisdn.is_empty() {
                println!("Logged in");
            } else {
                println!("Logged in as +34 {}", config.msisdn);
            }
        }

        Commands::Status => match config::load_auth() {
            Ok(cfg) => {
                // Check the session against the server, without renewing it.
                let session = match api::O2Client::new(&cfg.device_id) {
                    Ok(client) => match client.is_session_valid(&cfg).await {
                        Ok(true) => "active".to_string(),
                        Ok(false) if cfg.sapi_auth_header().is_some() => {
                            "expired (the next command renews it silently)".to_string()
                        }
                        Ok(false) => {
                            "expired (the next command opens the login window)".to_string()
                        }
                        Err(e) => format!("unknown — couldn't reach O2 Cloud ({e})"),
                    },
                    Err(e) => format!("unknown ({e})"),
                };
                println!("✓ Logged in");
                if !cfg.msisdn.is_empty() {
                    println!("  Phone: +34 {}", cfg.msisdn);
                }
                if cfg.logged_in_at > 0 {
                    println!("  Since: {}", util::format_unix_utc(cfg.logged_in_at));
                }
                println!("  Session: {}", session);
                if cfg.sapi_auth_header().is_none() {
                    println!("  Sessions last about 24 h and aren't renewed automatically.");
                }
            }
            Err(_) => {
                println!("✗ Not logged in. Run `o2cloud login` to authenticate.");
            }
        },

        Commands::Logout => {
            config::clear_auth()?;
            println!("✓ Logged out — tokens removed");
        }

        Commands::Ls { path, all, tree } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::list_folder(&client, &auth, path, all, tree).await?;
        }

        Commands::UploadDir { path, folder } => {
            let (client, auth) = require_client_and_auth().await?;
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                return Err(O2Error::Invalid(format!("not a directory: {}", path)));
            }
            ops::upload_dir(&client, &auth, &local_path, folder).await?;
        }

        Commands::UploadZip { path, folder } => {
            let (client, auth) = require_client_and_auth().await?;
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                return Err(O2Error::Invalid(format!("not a directory: {}", path)));
            }
            ops::upload_zip(&client, &auth, &local_path, folder).await?;
        }

        Commands::Rm {
            target,
            recursive,
            yes,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::delete_target(&client, &auth, &target, recursive, yes).await?;
        }

        Commands::Upload { path, folder } => {
            let (client, auth) = require_client_and_auth().await?;
            let local_path = PathBuf::from(&path);
            if !local_path.exists() {
                return Err(O2Error::NotFound(format!("file not found: {}", path)));
            }
            ops::upload_file(&client, &auth, &local_path, folder).await?;
        }

        Commands::Find { query } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::find(&client, &auth, &query).await?;
        }

        Commands::Share { target } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::share(&client, &auth, &target).await?;
        }

        Commands::Shares => {
            let (client, auth) = require_client_and_auth().await?;
            ops::list_shares(&client, &auth).await?;
        }

        Commands::Unshare { target } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::unshare(&client, &auth, &target).await?;
        }

        Commands::Mv {
            source,
            destination,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::move_item(&client, &auth, &source, &destination).await?;
        }

        Commands::Rename { target, new_name } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::rename_item(&client, &auth, &target, &new_name).await?;
        }

        Commands::Download { target, output } => {
            let (client, auth) = require_client_and_auth().await?;
            let output_path = output.map(PathBuf::from);
            ops::download_file(&client, &auth, &target, output_path).await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
