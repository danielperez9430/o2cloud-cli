mod api;
mod auth;
mod cache;
mod config;
mod error;
mod events;
mod filetype;
mod jpeg;
mod log;
mod ops;
mod redact;
mod report;
mod ui;
mod util;
mod webview;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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

    /// Show what the command does step by step (requests, retries, the
    /// decision for each file) on stderr, and keep it in the run log in
    /// full. Session keys never appear
    #[arg(long, global = true)]
    debug: bool,

    /// Show a line for each file (uploaded, skipped, each part…); by
    /// default long transfers show a progress bar and a summary
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Show only warnings and errors
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    quiet: bool,

    /// For apps: print events as they happen, one JSON object per line
    /// (start, progress, file, retry, summary, error), instead of one
    /// JSON value at the end. For upload, upload-dir, upload-zip,
    /// download and download-dir
    #[arg(long, global = true)]
    json_lines: bool,
}

/// Options shared by the upload commands.
#[derive(Args)]
struct UploadFlags {
    /// What to do when the target folder already has a file of that name
    /// with another size: ask (the default at a terminal), skip, overwrite
    /// (the old copy goes to the trash), fail (don't upload it and report
    /// it with exit code 5: the default without a terminal, e.g. for
    /// apps), or duplicate (upload everything anyway, as before 1.0: O2
    /// Cloud names the new one `name (1)`). A file there with the same
    /// name and size is always skipped, except with duplicate.
    #[arg(long, value_enum, value_name = "WHAT")]
    on_conflict: Option<ops::OnConflict>,

    /// The same as --on-conflict skip
    #[arg(long)]
    skip_existing: bool,

    /// Size in bytes above which a file is uploaded in parts (default:
    /// just under the server's 4 GB limit). For testing.
    #[arg(long, hide = true)]
    part_size: Option<u64>,
}

impl UploadFlags {
    fn options(&self, jobs: usize) -> ops::UploadOptions {
        use ops::OnConflict;
        let on_conflict = match (self.on_conflict, self.skip_existing) {
            (Some(OnConflict::Ask), _) | (None, false) if ops::can_ask() => OnConflict::Ask,
            (Some(OnConflict::Ask), _) | (None, false) => OnConflict::Fail,
            (Some(policy), _) => policy,
            (None, true) => OnConflict::Skip,
        };
        ops::UploadOptions {
            skip_existing: on_conflict != OnConflict::Duplicate,
            on_conflict,
            jobs,
            part_size: self
                .part_size
                .or_else(ops::test_part_size)
                .unwrap_or(ops::DEFAULT_PART_SIZE)
                .max(2),
            update: false,
            dry_run: false,
        }
    }
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

        /// Advanced: OAuth client id of the official mobile app, to log in
        /// with a session that renews without SMS. You must supply your own
        /// (needs --oauth-client-secret too). See the README.
        #[arg(
            long,
            env = "O2CLOUD_OAUTH_CLIENT_ID",
            hide = true,
            hide_env_values = true,
            requires = "oauth_client_secret"
        )]
        oauth_client_id: Option<String>,

        /// Advanced: OAuth client secret paired with --oauth-client-id.
        #[arg(
            long,
            env = "O2CLOUD_OAUTH_CLIENT_SECRET",
            hide = true,
            hide_env_values = true,
            requires = "oauth_client_id"
        )]
        oauth_client_secret: Option<String>,
    },

    /// Show current login status
    Status {
        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Log out (clear stored tokens)
    Logout,

    /// List files (default: root folder contents)
    Ls {
        /// Folder path or ID (e.g. /Photos/2024 or 12345678)
        path: Option<String>,

        /// Show every file under the folder (all folders by default), as a flat list
        #[arg(short = 'a', long)]
        all: bool,

        /// Tree view (hierarchical)
        #[arg(short = 't', long, conflicts_with_all = ["all", "json", "limit"])]
        tree: bool,

        /// Sort files by name, size or date
        #[arg(short = 's', long, value_enum, default_value_t = ops::SortKey::Name)]
        sort: ops::SortKey,

        /// Reverse the sort order
        #[arg(short = 'r', long)]
        reverse: bool,

        /// Long format: also show each file's date (when it was taken or
        /// created, else when it was uploaded; UTC)
        #[arg(short = 'l', long)]
        long: bool,

        /// Only show names matching this pattern (`*` and `?` wildcards,
        /// case-insensitive), e.g. '*.heic'
        #[arg(short = 'n', long = "name", value_name = "PATTERN")]
        pattern: Option<String>,

        /// Show at most N files
        #[arg(long, value_name = "N")]
        limit: Option<usize>,

        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,

        /// Only the files directly in the folder, without listing the
        /// whole account: much faster on a large one, but subfolders show
        /// no file count or size (null in JSON), and the root leaves out
        /// the photos and videos the phone app uploaded (see --no-folder)
        #[arg(long, conflicts_with_all = ["all", "tree"])]
        folder_only: bool,

        /// Only the files with no folder (photos and videos the phone app
        /// uploaded), which `ls` shows in the root. Lists the whole account
        #[arg(long, conflicts_with_all = ["path", "all", "tree", "folder_only"])]
        no_folder: bool,
    },

    /// Upload a file to O2 Cloud (files over 4 GB go up in parts,
    /// name.001, name.002, …, which `download name` joins again)
    Upload {
        /// Local files to upload. The last one can instead be the folder
        /// in the cloud to put them in, as a path starting with `/`
        /// (`upload a.jpg b.jpg /Photos/2024`), like --folder
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,

        /// Target folder: its ID or path, e.g. /Photos/2024 (omit for the
        /// root)
        #[arg(short, long)]
        folder: Option<String>,

        #[command(flatten)]
        upload: UploadFlags,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Upload a directory recursively
    UploadDir {
        /// Local directory path
        path: String,

        /// Target folder: its ID or path, e.g. /Photos/2024 (omit for the
        /// root)
        #[arg(short, long)]
        folder: Option<String>,

        #[command(flatten)]
        upload: UploadFlags,

        /// Files uploaded at the same time
        #[arg(short, long, default_value_t = 2, value_parser = clap::value_parser!(u16).range(1..=8))]
        jobs: u16,

        /// Only upload files with these extensions (comma-separated or
        /// repeated, any case, with or without the dot), e.g.
        /// --ext jpg,mp4 or --ext tar.gz. Subfolders with no such file
        /// aren't created
        #[arg(long, value_delimiter = ',', value_name = "EXT")]
        ext: Vec<String>,

        /// Also upload files that changed locally (another size, or a
        /// newer modification date than in the cloud) and move the cloud's
        /// old copy to the trash; a file older locally is left alone.
        /// With it, --on-conflict doesn't apply
        #[arg(long)]
        update: bool,

        /// With --update: also move to O2 Cloud's trash what the cloud
        /// folder has and the local directory no longer has (files and
        /// folders, only inside the uploaded folder), so the cloud mirrors
        /// it. Hidden and system files are never deleted. Check first with
        /// --dry-run -v
        #[arg(long, requires = "update", conflicts_with = "ext")]
        delete: bool,

        /// The most files and folders --delete may move to the trash in
        /// one run (default 1000; without it, also no more than half the
        /// cloud folder's files): a safeguard against mirroring the wrong
        /// directory. Over it, nothing is done
        #[arg(long, value_name = "N", requires = "delete")]
        delete_max: Option<usize>,

        /// Only say what would be uploaded, updated or deleted; nothing is
        /// created, uploaded or moved to the trash
        #[arg(long)]
        dry_run: bool,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Zip a directory and upload it as a single file
    UploadZip {
        /// Local directory path
        path: String,

        /// Target folder: its ID or path, e.g. /Photos/2024 (omit for the
        /// root)
        #[arg(short, long)]
        folder: Option<String>,

        #[command(flatten)]
        upload: UploadFlags,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Delete a file or folder (move to trash)
    Rm {
        /// Files or folders: IDs or paths (e.g. 1234567890 /Photos/a.jpg)
        #[arg(required = true, num_args = 1..)]
        targets: Vec<String>,

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

        /// Print JSON instead of a list
        #[arg(long)]
        json: bool,
    },

    /// Create a public link to a file or folder and print its URL
    Share {
        /// File/folder ID, path (e.g. /docs or /docs/a.txt), or unique file name
        target: String,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// List shared folders (public links)
    Shares {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },

    /// Remove the public link of a folder
    Unshare {
        /// Folder ID or path, or a link ID from `o2cloud shares`
        target: String,
    },

    /// Move a file or folder into another folder
    Mv {
        /// File/folder ID, path (e.g. /docs/a.txt or /docs), or unique file name
        source: String,

        /// Destination folder ID or path (e.g. /archive)
        destination: String,
    },

    /// Copy a file into a folder, on the server (nothing is downloaded)
    Cp {
        /// File ID, path (e.g. /docs/a.txt), or unique file name
        source: String,

        /// Destination folder ID or path (e.g. /archive)
        destination: String,

        /// Name for the copy (default: the same name)
        #[arg(short, long)]
        name: Option<String>,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Rename a file or folder
    Rename {
        /// File/folder ID, path (e.g. /docs/a.txt or /docs), or unique file name
        target: String,

        /// New name (just the name, not a path)
        new_name: String,
    },

    /// Create a folder
    Mkdir {
        /// Folder path (e.g. /Photos/2024)
        path: String,

        /// Create missing parent folders too, and don't fail if the folder exists
        #[arg(short, long)]
        parents: bool,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Find files whose extension doesn't match what they are (photos,
    /// videos, audio and PDFs: a WebP or HEIC photo named .jpg, a MOV video
    /// named .mp4, an M4A named .mp3), which the computer opens with the
    /// wrong program or not at all and O2 Cloud may reject, and JPEGs whose
    /// colour profile O2 Cloud can't read (upload fails with MED-1000); with
    /// --apply, rename the first to the right extension and repair the
    /// second, without touching their contents. Works on local files only;
    /// no login needed
    #[command(name = "fix", visible_alias = "fix-jpeg")]
    FixJpeg {
        /// Files or folders to check (folders are searched recursively)
        #[arg(required = true)]
        paths: Vec<String>,

        /// Rename and repair the files in place (keeping their dates);
        /// without it, only list them
        #[arg(long)]
        apply: bool,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Make a report to attach to a bug report: version, system, settings
    /// (no keys), and the logs of the last runs, with file and folder
    /// names replaced. Nothing is sent anywhere
    DebugReport {
        /// Where to write it (default: o2cloud-report-<run>.zip here)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Print the report as text instead of writing a zip
        #[arg(long)]
        stdout: bool,

        /// Keep file and folder names as they are
        #[arg(long)]
        include_names: bool,
    },

    /// List the albums and what O2 Cloud groups by itself (moments, places,
    /// and people once face recognition is turned on in its website); with
    /// a label's ID or name, its photos and videos
    Labels {
        /// A label's ID or name, to list its photos and videos
        label: Option<String>,

        /// Which labels to list
        #[arg(long, value_enum, default_value_t = ops::LabelKind::All)]
        kind: ops::LabelKind,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// List the account's photos, videos, music or documents, wherever
    /// they are
    Media {
        /// Which kind of files
        #[arg(value_enum)]
        kind: ops::MediaKind,

        /// Print JSON instead of a list (with each file's thumbnails,
        /// location, a song's duration, and a signed URL that opens the
        /// file without the session for a while: keep the output private)
        #[arg(long)]
        json: bool,
    },

    /// Show storage used, free and in the trash
    Quota {
        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Download a folder and everything in it (or with `/`, the whole
    /// account) into a local folder; files uploaded in parts are joined
    DownloadDir {
        /// Folder ID or path (e.g. /Photos), or / for the whole account
        #[arg(required_unless_present = "label")]
        target: Option<String>,

        /// Download a label instead of a folder: a moment, a place, a person
        /// or an album (its ID or name, see `o2cloud labels`), into a folder
        /// of its name
        #[arg(long, conflicts_with = "target", value_name = "LABEL")]
        label: Option<String>,

        /// Local folder to download into (default: the current one; needed
        /// for /)
        #[arg(short, long)]
        output: Option<String>,

        /// Files downloaded at the same time
        #[arg(short, long, default_value_t = 2, value_parser = clap::value_parser!(u16).range(1..=8))]
        jobs: u16,

        /// Kept for scripts from before 1.0: files already there with the
        /// same size are always skipped now, and cut downloads resumed
        /// (nothing local is ever overwritten)
        #[arg(long, hide = true)]
        skip_existing: bool,

        /// Only files with these extensions (comma-separated or repeated,
        /// any case, with or without the dot), e.g. --ext jpg,mp4
        #[arg(long, value_delimiter = ',', value_name = "EXT")]
        ext: Vec<String>,

        /// Only say how many files and how much would be downloaded
        #[arg(long)]
        dry_run: bool,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },

    /// Download a file from O2 Cloud
    Download {
        /// Media ID (shown in `o2cloud ls`), file path (e.g. /docs/a.txt),
        /// or file name if it is unique
        target: String,

        /// Output file path (defaults to current directory)
        #[arg(short, long)]
        output: Option<String>,

        /// Print JSON instead of messages
        #[arg(long)]
        json: bool,
    },
}

/// Load stored credentials and an API client. If the session has
/// expired, sessions from the old login are renewed silently with their
/// stored token; otherwise (or if that fails) the login window opens.
async fn require_client_and_auth() -> Result<(api::O2Client, config::AuthConfig), O2Error> {
    let auth = config::load_auth()?;
    let client = api::O2Client::new(&auth.device_id)?;

    let kind = if auth.sapi_auth_header().is_some() {
        "app"
    } else {
        "web"
    };
    if client.check_session(&auth).await? {
        log::info(
            "auth",
            "session valid",
            serde_json::json!({ "login": kind }),
        );
        return Ok((client, auth));
    }
    log::info(
        "auth",
        "session expired",
        serde_json::json!({ "login": kind }),
    );

    if auth.sapi_auth_header().is_some() {
        eprintln!("→ Session expired, trying silent re-login...");
        match client.silent_login(&auth).await {
            Ok(login_resp) => {
                // Renewal gives a fresh JSESSIONID + validationkey. The
                // renewal token itself does not rotate (see the native
                // flow: it returns the same `access_token`), so if the
                // response omits it, keep the one we already have rather
                // than overwrite it with an empty string — losing it
                // would leave the session unrenewable until the next SMS
                // login. Same for the encryption token.
                let access_token = if login_resp.access_token.is_empty() {
                    auth.access_token.clone()
                } else {
                    login_resp.access_token
                };
                let encryption_token = if login_resp.encryption_token.is_empty() {
                    auth.encryption_token.clone()
                } else {
                    login_resp.encryption_token
                };
                let new_auth = config::AuthConfig {
                    validationkey: login_resp.validationkey,
                    jsessionid: login_resp.jsessionid,
                    access_token,
                    encryption_token,
                    msisdn: auth.msisdn.clone(),
                    platform: auth.platform.clone(),
                    device_id: client.device_id().to_string(),
                    logged_in_at: config::now_unix(),
                    oauth_client: auth.oauth_client.clone(),
                };
                if let Err(e) = config::save_auth(&new_auth) {
                    eprintln!("Failed to save refreshed auth: {}", e);
                }
                eprintln!("✓ Session refreshed silently");
                log::info("auth", "renewed silently", serde_json::Value::Null);
                return Ok((client, new_auth));
            }
            Err(e) => {
                eprintln!("→ Silent re-login failed ({e})");
                log::warn(
                    "auth",
                    "silent renewal failed",
                    serde_json::json!({ "error": e.to_string() }),
                );
            }
        }
    } else {
        eprintln!("→ Session expired");
    }

    // The login window only opens by itself for someone at a terminal:
    // a script or an app gets "Session expired" (exit 4) and logs in with
    // `o2cloud login` when it chooses. O2CLOUD_LOGIN_WINDOW=1 opens it
    // anyway.
    if !interactive() {
        log::info(
            "auth",
            "no terminal: not opening the login window",
            serde_json::Value::Null,
        );
        return Err(O2Error::SessionExpired);
    }
    log::info("auth", "opening the login window", serde_json::Value::Null);
    let new_auth = login_like(&auth).await?;
    eprintln!("✓ Re-login successful");
    let new_client = api::O2Client::new(&new_auth.device_id)?;
    Ok((new_client, new_auth))
}

/// Take the destination off `upload`'s arguments: the last one, if there
/// are several and it's a cloud path (starts with `/`) rather than a local
/// file.
fn split_destination(paths: &mut Vec<String>) -> Option<String> {
    match paths.last() {
        Some(last)
            if paths.len() > 1
                && last.starts_with('/')
                && !std::path::Path::new(last).is_file() =>
        {
            paths.pop()
        }
        _ => None,
    }
}

/// Why `--json-lines` can't be used with this command line, if it can't.
fn json_lines_problem(cli: &Cli) -> Option<&'static str> {
    if !cli.json_lines {
        return None;
    }
    if std::env::args().any(|a| a == "--json") {
        return Some("use --json or --json-lines, not both");
    }
    match cli.command {
        Commands::Upload { .. }
        | Commands::UploadDir { .. }
        | Commands::UploadZip { .. }
        | Commands::Download { .. }
        | Commands::DownloadDir { .. } => None,
        _ => Some(
            "--json-lines is for upload, upload-dir, upload-zip, download and download-dir; \
             use --json with other commands",
        ),
    }
}

/// Whether someone at a terminal runs the CLI (stdin is one), or
/// `O2CLOUD_LOGIN_WINDOW=1` says to behave as if so.
fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() || std::env::var("O2CLOUD_LOGIN_WINDOW").is_ok_and(|v| v == "1")
}

/// Log in again the way `old` was logged in: a session from the mobile-app
/// login (which renews without SMS) gets that login again, with the OAuth
/// client from the environment or the one stored with it. Without one it
/// says so and falls back to the web login.
async fn login_like(old: &config::AuthConfig) -> Result<config::AuthConfig, O2Error> {
    let was_app_login = old.oauth_client.is_some() || old.platform == "ios";
    if !was_app_login {
        // Logging in with browser cookies gives the same kind of session;
        // for an app session it would give a web one, so it isn't offered.
        eprintln!(
            "  (without a login window, run `o2cloud login --jsessionid ... \
             --validationkey ...` instead)"
        );
        return crate::auth::login().await;
    }
    match config::OAuthClient::from_env().or_else(|| old.oauth_client.clone()) {
        Some(c) => crate::auth::login_native(&c.id, &c.secret).await,
        None => {
            eprintln!(
                "⚠ This session came from the mobile-app login, but its OAuth client isn't \
                 stored or set (O2CLOUD_OAUTH_CLIENT_ID / O2CLOUD_OAUTH_CLIENT_SECRET), so \
                 this is the web login: its sessions end after a while without use."
            );
            crate::auth::login().await
        }
    }
}

async fn run(cli: Cli) -> Result<(), O2Error> {
    match cli.command {
        Commands::Login {
            phone,
            jsessionid,
            validationkey,
            oauth_client_id,
            oauth_client_secret,
        } => {
            if phone.is_some() {
                eprintln!(
                    "Note: --phone is no longer needed; enter the number in the login window."
                );
            }
            let config = match (
                oauth_client_id,
                oauth_client_secret,
                jsessionid,
                validationkey,
            ) {
                (Some(client_id), Some(client_secret), _, _) => {
                    auth::login_native(&client_id, &client_secret).await?
                }
                (_, _, Some(jsessionid), Some(validationkey)) => {
                    auth::login_manual(&jsessionid, &validationkey).await?
                }
                // Same kind of login as the stored session, if any.
                _ => match config::load_auth() {
                    Ok(old) => login_like(&old).await?,
                    Err(_) => auth::login().await?,
                },
            };
            if config.msisdn.is_empty() {
                println!("Logged in");
            } else {
                println!("Logged in as +34 {}", config.msisdn);
            }
        }

        Commands::Status { json } => match config::load_auth() {
            Ok(cfg) => {
                // Check the session against the server, without renewing it.
                let mut storage = None;
                // The state for `--json`, and how to word it for people.
                let (state, session) = match api::O2Client::new(&cfg.device_id) {
                    Ok(client) => match client.is_session_valid(&cfg).await {
                        Ok(true) => {
                            storage = client.storage_space(&cfg).await.ok();
                            ("active", "active".to_string())
                        }
                        Ok(false) if cfg.sapi_auth_header().is_some() => (
                            "expired",
                            "expired (the next command renews it silently)".to_string(),
                        ),
                        Ok(false) => (
                            "expired",
                            "expired (log in again with `o2cloud login`)".to_string(),
                        ),
                        Err(e) => (
                            "unknown",
                            format!("unknown — couldn't reach O2 Cloud ({e})"),
                        ),
                    },
                    Err(e) => ("unknown", format!("unknown ({e})")),
                };
                if json {
                    ops::print_json(&serde_json::json!({
                        "logged_in": true,
                        "phone": (!cfg.msisdn.is_empty()).then(|| format!("+34{}", cfg.msisdn)),
                        "since": ops::format_date_iso(
                            (cfg.logged_in_at > 0).then(|| cfg.logged_in_at * 1000)
                        ),
                        "session": state,
                        "renews_silently": cfg.sapi_auth_header().is_some(),
                        "login": if cfg.sapi_auth_header().is_some() { "app" } else { "web" },
                        "storage": storage,
                    }));
                    return Ok(());
                }
                println!("✓ Logged in");
                if !cfg.msisdn.is_empty() {
                    println!("  Phone: +34 {}", cfg.msisdn);
                }
                if cfg.logged_in_at > 0 {
                    println!("  Since: {}", util::format_unix_utc(cfg.logged_in_at));
                }
                println!("  Session: {}", session);
                if cfg.sapi_auth_header().is_some() {
                    println!("  Login: app (the session renews itself without SMS)");
                } else {
                    println!("  Login: web (the session ends after a while without use)");
                }
                if let Some(space) = storage {
                    if space.nolimit {
                        println!("  Storage: {} used", util::format_size(space.used));
                    } else {
                        println!(
                            "  Storage: {} of {} used",
                            util::format_size(space.used),
                            util::format_size(space.quota)
                        );
                    }
                }
            }
            Err(_) if json => {
                ops::print_json(&serde_json::json!({ "logged_in": false }));
            }
            Err(_) => {
                println!("✗ Not logged in. Run `o2cloud login` to authenticate.");
            }
        },

        Commands::DebugReport {
            output,
            stdout,
            include_names,
        } => report::debug_report(output, stdout, include_names)?,

        Commands::Logout => {
            config::clear_auth()?;
            // The run logs name the account's files too.
            if let Some(dir) = log::dir() {
                let _ = std::fs::remove_dir_all(dir);
            }
            // The cache lists the account's file names.
            if let Err(e) = cache::clear() {
                eprintln!("Failed to remove the file cache: {}", e);
            }
            println!("✓ Logged out — tokens removed");
        }

        Commands::Ls {
            path,
            all,
            tree,
            sort,
            reverse,
            long,
            pattern,
            limit,
            json,
            folder_only,
            no_folder,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            let opts = ops::ListOptions {
                all,
                tree,
                folder_only,
                no_folder,
                sort,
                reverse,
                long,
                pattern,
                limit,
                json,
            };
            ops::list_folder(&client, &auth, path, &opts).await?;
        }

        Commands::UploadDir {
            path,
            folder,
            upload,
            jobs,
            ext,
            update,
            delete,
            delete_max,
            dry_run,
            json,
        } => {
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                return Err(O2Error::Invalid(format!("not a directory: {}", path)));
            }
            let extensions = ops::normalize_extensions(&ext);
            if !ext.is_empty() && extensions.is_empty() {
                return Err(O2Error::Invalid(
                    "--ext needs at least one extension".into(),
                ));
            }
            let (client, auth) = require_client_and_auth().await?;
            let folder = ops::folder_arg(&client, &auth, folder.as_deref()).await?;
            ops::upload_dir(
                &client,
                &auth,
                &local_path,
                folder,
                &{
                    let base = upload.options(jobs.into());
                    ops::UploadOptions {
                        skip_existing: base.skip_existing || update,
                        update,
                        dry_run,
                        ..base
                    }
                },
                &extensions,
                json,
                delete.then(|| {
                    (
                        delete_max.unwrap_or(ops::DEFAULT_DELETE_MAX),
                        delete_max.is_some(),
                    )
                }),
            )
            .await?;
        }

        Commands::UploadZip {
            path,
            folder,
            upload,
            json,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            let local_path = PathBuf::from(&path);
            if !local_path.is_dir() {
                return Err(O2Error::Invalid(format!("not a directory: {}", path)));
            }
            let folder = ops::folder_arg(&client, &auth, folder.as_deref()).await?;
            ops::upload_zip(
                &client,
                &auth,
                &local_path,
                folder,
                &upload.options(1),
                json,
            )
            .await?;
        }

        Commands::Rm {
            targets,
            recursive,
            yes,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            // The folder list once for all; each target in turn, going on
            // past one that fails.
            let folders = client.get_all_folders(&auth).await?;
            let mut first_error = None;
            let mut failed = 0;
            for target in &targets {
                if let Err(e) =
                    ops::delete_target(&client, &auth, target, recursive, yes, &folders).await
                {
                    if matches!(e, O2Error::SessionExpired) {
                        return Err(e);
                    }
                    if targets.len() > 1 {
                        crate::warn!("  ⚠ {}: {}", target, redact::text(&e.to_string()));
                    }
                    failed += 1;
                    first_error.get_or_insert(e);
                }
            }
            match first_error {
                // None deleted: what went wrong, as it is.
                Some(e) if failed == targets.len() => return Err(e),
                Some(_) => {
                    return Err(O2Error::Partial(format!(
                        "{} of {} could not be deleted",
                        failed,
                        targets.len()
                    )))
                }
                None => {}
            }
        }

        Commands::Upload {
            mut paths,
            folder,
            upload,
            json,
        } => {
            let destination = split_destination(&mut paths);
            if destination.is_some() && folder.is_some() {
                return Err(O2Error::Invalid(
                    "give the destination once: as the last argument or with --folder".into(),
                ));
            }
            let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            for (file, given) in files.iter().zip(&paths) {
                if file.is_dir() {
                    return Err(O2Error::Invalid(format!(
                        "{} is a folder: use `o2cloud upload-dir` (or `upload-zip`)",
                        given
                    )));
                }
                if !file.exists() {
                    return Err(O2Error::NotFound(format!("file not found: {}", given)));
                }
            }
            let (client, auth) = require_client_and_auth().await?;
            let given = destination.is_some();
            let folder = ops::folder_arg(&client, &auth, destination.or(folder).as_deref())
                .await
                .map_err(|e| match e {
                    // Say why it was looked up in the cloud.
                    O2Error::NotFound(msg) if given => O2Error::NotFound(format!(
                        "{} (the last argument starts with `/`, so it was taken as the folder \
                         in the cloud to upload to; it must exist)",
                        msg
                    )),
                    e => e,
                })?;
            ops::upload_files(
                &client,
                &auth,
                &files,
                folder,
                &upload.options(1),
                json,
                "upload",
            )
            .await?;
        }

        Commands::Find { query, json } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::find(&client, &auth, &query, json).await?;
        }

        Commands::Share { target, json } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::share(&client, &auth, &target, json).await?;
        }

        Commands::Shares { json } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::list_shares(&client, &auth, json).await?;
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

        Commands::Cp {
            source,
            destination,
            name,
            json,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::copy_item(&client, &auth, &source, &destination, name.as_deref(), json).await?;
        }

        Commands::Mkdir {
            path,
            parents,
            json,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::make_folder(&client, &auth, &path, parents, json).await?;
        }

        Commands::FixJpeg { paths, apply, json } => {
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            ops::fix_jpeg(&paths, apply, json)?;
        }

        Commands::DownloadDir {
            target,
            label,
            output,
            jobs,
            skip_existing,
            ext,
            dry_run,
            json,
        } => {
            let extensions = ops::normalize_extensions(&ext);
            if !ext.is_empty() && extensions.is_empty() {
                return Err(O2Error::Invalid(
                    "--ext needs at least one extension".into(),
                ));
            }
            let (client, auth) = require_client_and_auth().await?;
            // Always on since 1.0; the flag stays so old scripts still run.
            let _ = skip_existing;
            let opts = ops::DownloadDirOptions {
                jobs: jobs.into(),
                skip_existing: true,
                extensions,
                dry_run,
            };
            match (label, target) {
                (Some(label), _) => {
                    let label = ops::find_label(&client, &auth, &label).await?;
                    let media = ops::label_media(&client, &auth, &label).await?;
                    let name = label.name.clone().unwrap_or_else(|| label.id.to_string());
                    ops::download_label(
                        &client,
                        &auth,
                        &name,
                        &media,
                        output.map(PathBuf::from),
                        &opts,
                        json,
                    )
                    .await?;
                }
                (None, Some(target)) => {
                    ops::download_dir(
                        &client,
                        &auth,
                        &target,
                        output.map(PathBuf::from),
                        &opts,
                        json,
                    )
                    .await?;
                }
                (None, None) => unreachable!("clap requires one"),
            }
        }

        Commands::Labels { label, kind, json } => {
            let (client, auth) = require_client_and_auth().await?;
            match label {
                Some(label) => ops::show_label(&client, &auth, &label, json).await?,
                None => ops::list_labels(&client, &auth, kind, json).await?,
            }
        }

        Commands::Media { kind, json } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::list_media_kind(&client, &auth, kind, json).await?;
        }

        Commands::Quota { json } => {
            let (client, auth) = require_client_and_auth().await?;
            ops::show_quota(&client, &auth, json).await?;
        }

        Commands::Download {
            target,
            output,
            json,
        } => {
            let (client, auth) = require_client_and_auth().await?;
            let output_path = output.map(PathBuf::from);
            ops::download_file(&client, &auth, &target, output_path, json).await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    // Exit quietly when stdout is closed early (`o2cloud ls | head`)
    // instead of panicking on the write: Rust ignores SIGPIPE by default.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();
    // Commands that only look don't keep a log (unless --debug): they'd
    // push the logs of transfers out of the last ten.
    let only_looks = matches!(
        cli.command,
        Commands::Ls { .. }
            | Commands::Find { .. }
            | Commands::Status { .. }
            | Commands::Quota { .. }
            | Commands::Shares { .. }
            | Commands::Labels { .. }
            | Commands::Media { .. }
            | Commands::DebugReport { .. }
    );
    log::init(cli.debug, !only_looks);
    ui::set_level(cli.quiet, cli.verbose);
    if cli.json_lines {
        events::enable();
    }
    // A crash leaves a report next to the run logs.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error(
            "run",
            "crash",
            serde_json::json!({ "panic": info.to_string() }),
        );
        log::flush();
        default_hook(info);
        if let Some(path) = log::crash_report(info) {
            eprintln!(
                "o2cloud crashed. A report is in {}; please attach it (or the output of \
                 `o2cloud debug-report`) to an issue at \
                 https://github.com/danielperez9430/o2cloud-cli/issues",
                path.display()
            );
        }
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    log::info(
        "run",
        "start",
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "command": args.iter().find(|a| !a.starts_with('-')),
            "args": redact::args(&args),
            "interactive": ops::can_ask(),
        }),
    );
    let result = match json_lines_problem(&cli) {
        Some(problem) => Err(O2Error::Invalid(problem.into())),
        None => run(cli).await,
    };
    if api::tracing() {
        eprintln!("[trace] {} requests", api::requests_sent());
    }
    let (exit_code, error) = match &result {
        Ok(()) => (0, None),
        Err(e) => (e.exit_code(), Some((e.code(), e.to_string()))),
    };
    if let Some((code, message)) = &error {
        log::error("run", message, serde_json::json!({ "code": code }));
    }
    log::info(
        "run",
        "end",
        serde_json::json!({ "exit_code": exit_code, "requests": api::requests_sent() }),
    );
    log::flush();
    if let Err(e) = result {
        // The run ID finds this run's log (`o2cloud debug-report`).
        let message = redact::text(&e.to_string());
        if log::path().is_some() {
            eprintln!("Error: {message} [run {}]", log::run_id());
        } else {
            eprintln!("Error: {message}");
        }
        // With --json, the error as JSON on stdout too, where the result
        // would have gone.
        if events::on() {
            events::emit("error", e.to_json()["error"].clone());
        } else if std::env::args().any(|a| a == "--json")
            && !ops::PRINTED_JSON.load(std::sync::atomic::Ordering::Relaxed)
        {
            println!("{}", e.to_json());
        }
        std::process::exit(e.exit_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_destination() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut p = v(&["a.jpg", "b.jpg", "/Fotos/2024"]);
        assert_eq!(split_destination(&mut p).as_deref(), Some("/Fotos/2024"));
        assert_eq!(p, v(&["a.jpg", "b.jpg"]));
        // One argument is always a file to upload.
        let mut p = v(&["/Fotos"]);
        assert_eq!(split_destination(&mut p), None);
        // The last one isn't a cloud path.
        let mut p = v(&["a.jpg", "b.jpg"]);
        assert_eq!(split_destination(&mut p), None);
        // A local file given by its absolute path is a file, not a folder.
        let here = std::env::current_exe().unwrap().display().to_string();
        if here.starts_with('/') {
            let mut p = v(&["a.jpg", &here]);
            assert_eq!(split_destination(&mut p), None);
        }
    }

    #[test]
    fn test_upload_options() {
        use ops::OnConflict;
        let flags = |on_conflict, skip_existing| UploadFlags {
            on_conflict,
            skip_existing,
            part_size: Some(100),
        };
        // Tests run without a terminal: no one to ask, so fail.
        let o = flags(None, false).options(1);
        assert_eq!((o.on_conflict, o.skip_existing), (OnConflict::Fail, true));
        let o = flags(Some(OnConflict::Ask), false).options(1);
        assert_eq!(o.on_conflict, OnConflict::Fail);
        let o = flags(None, true).options(1);
        assert_eq!(o.on_conflict, OnConflict::Skip);
        let o = flags(Some(OnConflict::Overwrite), true).options(1);
        assert_eq!(o.on_conflict, OnConflict::Overwrite);
        // Only duplicate uploads what's already there again.
        let o = flags(Some(OnConflict::Duplicate), false).options(1);
        assert!(!o.skip_existing);
    }
}
