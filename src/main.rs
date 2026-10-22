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
    },

    /// Show current login status
    Status,

    /// Log out (clear stored tokens)
    Logout,

    /// List files (default: root folder contents)
    Ls {
        /// Folder path or ID (e.g. /DMHAIR/app or 24440698)
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
        /// File/folder ID or path (e.g. 1195003130 or /DMHAIR)
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

    eprintln!("→ Opening login window...");
    let new_auth = crate::auth::login().await?;
    eprintln!("✓ Re-login successful");
    let new_client = api::O2Client::new(&new_auth.device_id)?;
    Ok((new_client, new_auth))
}

async fn run(cli: Cli) -> Result<(), O2Error> {
    match cli.command {
        Commands::Login { phone } => {
            if phone.is_some() {
                eprintln!(
                    "Note: --phone is no longer needed; enter the number in the login window."
                );
            }
            let config = auth::login().await?;
            if config.msisdn.is_empty() {
                println!("Logged in");
            } else {
                println!("Logged in as +34 {}", config.msisdn);
            }
        }

        Commands::Status => match config::load_auth() {
            Ok(cfg) => {
                println!("✓ Logged in");
                if !cfg.msisdn.is_empty() {
                    println!("  Phone: +34 {}", cfg.msisdn);
                }
                println!("  Platform: {}", cfg.platform);
                println!(
                    "  Validation key: {}...",
                    &cfg.validationkey[..16.min(cfg.validationkey.len())]
                );
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
                return Err(O2Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("not a directory: {}", path),
                )));
            }
            ops::upload_dir(&client, &auth, &local_path, folder).await?;
        }

        Commands::UploadZip { path, folder } => {
            let (client, auth) = require_client_and_auth().await?;
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                return Err(O2Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("not a directory: {}", path),
                )));
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
