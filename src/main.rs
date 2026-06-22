mod api;
mod auth;
mod cache;
mod config;
mod error;
mod ops;
mod webview;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    /// Log in to O2 Cloud via Mobile Connect (opens a webview window)
    Login {
        /// Spanish phone number (without country code, e.g. 686942006)
        #[arg(short, long)]
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
    },

    /// Search files and folders by name
    Find {
        /// Search query (case-insensitive substring match)
        query: String,
    },

    /// Download a file from O2 Cloud by media ID
    Download {
        /// Media ID (shown in `o2cli ls`)
        id: u64,

        /// Output file path (defaults to current directory)
        #[arg(short, long)]
        output: Option<String>,
    },
}

async fn require_client_and_auth() -> (api::O2Client, config::AuthConfig) {
    let auth = match config::load_auth() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let client = match api::O2Client::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to create API client: {}", e);
            std::process::exit(1);
        }
    };
    if !client.is_session_valid(&auth).await {
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
                };
                if let Err(e) = config::save_auth(&new_auth) {
                    eprintln!("Failed to save refreshed auth: {}", e);
                }
                eprintln!("✓ Session refreshed silently");
                return (client, new_auth);
            }
            Err(_) => {
                eprintln!("→ Silent re-login failed, opening login window...");
                let msisdn = auth.msisdn.strip_prefix("34").unwrap_or(&auth.msisdn).to_string();
                match crate::auth::login(&msisdn).await {
                    Ok(new_auth) => {
                        eprintln!("✓ Re-login successful");
                        let new_client = match api::O2Client::new() {
                            Ok(c) => c,
                            Err(e2) => {
                                eprintln!("Failed to create API client: {}", e2);
                                std::process::exit(1);
                            }
                        };
                        return (new_client, new_auth);
                    }
                    Err(e2) => {
                        eprintln!("Re-login failed: {}", e2);
                        std::process::exit(1);
                    }
                }
            }
        }
    }
    (client, auth)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Login { phone } => {
            let phone = match phone {
                Some(p) => p,
                None => {
                    eprint!("Enter your phone number (+34): ");
                    let mut input = String::new();
                    std::io::stdin()
                        .read_line(&mut input)
                        .expect("Failed to read input");
                    input.trim().to_string()
                }
            };

            if phone.is_empty() {
                eprintln!("Error: phone number is required");
                std::process::exit(1);
            }

            match auth::login(&phone).await {
                Ok(config) => {
                    println!("Logged in as +34 {}", config.msisdn);
                }
                Err(e) => {
                    eprintln!("Login failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Status => match config::load_auth() {
            Ok(cfg) => {
                println!("✓ Logged in");
                println!("  Phone: +34 {}", cfg.msisdn);
                println!("  Platform: {}", cfg.platform);
                println!(
                    "  Validation key: {}...",
                    &cfg.validationkey[..16.min(cfg.validationkey.len())]
                );
            }
            Err(_) => {
                println!("✗ Not logged in. Run `o2cli login` to authenticate.");
            }
        },

        Commands::Logout => match config::clear_auth() {
            Ok(()) => println!("✓ Logged out — tokens removed"),
            Err(e) => {
                eprintln!("Failed to clear auth: {}", e);
                std::process::exit(1);
            }
        },

        Commands::Ls { path, all, tree } => {
            let (client, auth) = require_client_and_auth().await;
            match ops::list_folder(&client, &auth, path, all, tree).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("List failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::UploadDir { path, folder } => {
            let (client, auth) = require_client_and_auth().await;
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                eprintln!("Error: not a directory: {}", path);
                std::process::exit(1);
            }
            match ops::upload_dir(&client, &auth, &local_path, folder).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Directory upload failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::UploadZip { path, folder } => {
            let (client, auth) = require_client_and_auth().await;
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                eprintln!("Error: not a directory: {}", path);
                std::process::exit(1);
            }
            match ops::upload_zip(&client, &auth, &local_path, folder).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Zip upload failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Rm { target, recursive } => {
            let (client, auth) = require_client_and_auth().await;
            match ops::delete_target(&client, &auth, &target, recursive).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Delete failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Upload { path, folder } => {
            let (client, auth) = require_client_and_auth().await;
            let local_path = PathBuf::from(&path);
            if !local_path.exists() {
                eprintln!("Error: file not found: {}", path);
                std::process::exit(1);
            }
            match ops::upload_file(&client, &auth, &local_path, folder).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Upload failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Find { query } => {
            let (client, auth) = require_client_and_auth().await;
            match ops::find(&client, &auth, &query).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Search failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Download { id, output } => {
            let (client, auth) = require_client_and_auth().await;
            let output_path = match output {
                Some(ref p) => PathBuf::from(p),
                None => PathBuf::from(format!("{}.downloaded", id)),
            };
            match ops::download_file(&client, &auth, id, &output_path).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Download failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
