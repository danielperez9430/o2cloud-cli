//! `debug-report`: what a bug report needs, gathered in one file the user
//! can look over and attach. It works offline and sends nothing.
//!
//! Session keys never get in (see `redact`). File and folder names are
//! personal too, so unless `--include-names` is given, every name in the
//! logs is replaced by a pseudonym that keeps what bugs about names need
//! (extension, length in bytes, accents, trailing dots, reserved names).

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::O2Error;

/// How many runs' logs go in.
const RUNS: usize = 3;
/// Log fields that hold names of files, folders or paths.
const NAME_FIELDS: [&str; 7] = ["name", "path", "local", "cloud", "folder", "file", "dir"];

pub fn debug_report(
    output: Option<PathBuf>,
    to_stdout: bool,
    include_names: bool,
) -> Result<(), O2Error> {
    let salt = format!(
        "{:x}",
        std::process::id() as u64 ^ crate::config::now_unix()
    );
    let summary = summary();
    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(dir) = crate::log::dir() {
        for path in newest(&dir, "run-", RUNS + 1)
            .into_iter()
            // Not this run's own log, which only says it made a report.
            .filter(|p| Some(p) != crate::log::path())
            .take(RUNS)
        {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let text = if include_names {
                home_to_tilde(&text)
            } else {
                pseudonymise_log(&text, &salt)
            };
            files.push((name_of(&path), text));
        }
        for path in newest(&dir, "crash-", 2) {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            files.push((name_of(&path), home_to_tilde(&text)));
        }
    }

    if to_stdout {
        println!("{}", summary);
        for (name, text) in &files {
            println!("\n===== {} =====\n{}", name, text.trim_end());
        }
        return Ok(());
    }
    let path = output
        .unwrap_or_else(|| PathBuf::from(format!("o2cloud-report-{}.zip", crate::log::run_id())));
    if path.exists() {
        return Err(O2Error::Conflict(format!(
            "{} already exists; not overwriting it",
            path.display()
        )));
    }
    let io = |e: std::io::Error| {
        O2Error::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {}", path.display(), e),
        ))
    };
    let zip_err = |e: zip::result::ZipError| O2Error::Io(std::io::Error::other(e.to_string()));
    let file = std::fs::File::create(&path).map_err(io)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("report.txt", options).map_err(zip_err)?;
    zip.write_all(summary.as_bytes()).map_err(io)?;
    for (name, text) in &files {
        zip.start_file(format!("logs/{}", name), options)
            .map_err(zip_err)?;
        zip.write_all(text.as_bytes()).map_err(io)?;
    }
    zip.finish().map_err(zip_err)?;
    eprintln!("✓ Report written to {}", path.display());
    eprintln!(
        "  It has the version, your system, settings (no keys) and the logs of the last {} \
         runs{}. Look it over before attaching it to an issue: \
         https://github.com/danielperez9430/o2cloud-cli/issues",
        RUNS,
        if include_names {
            ", with file and folder names as they are"
        } else {
            ", with file and folder names replaced"
        }
    );
    Ok(())
}

/// The report's first part: version, system, settings and session.
fn summary() -> String {
    use std::io::IsTerminal;
    let mut s = String::new();
    let mut line = |k: &str, v: String| s.push_str(&format!("{:<20} {}\n", k, v));
    line("o2cloud", env!("CARGO_PKG_VERSION").to_string());
    line(
        "system",
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    );
    line("os version", os_version());
    line(
        "locale",
        ["LC_ALL", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "-".into()),
    );
    line(
        "terminal",
        format!(
            "stdin {}, stderr {}",
            std::io::stdin().is_terminal(),
            std::io::stderr().is_terminal()
        ),
    );
    let mut env: Vec<String> = std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("O2CLOUD_"))
        .collect();
    env.sort();
    line(
        "O2CLOUD_* set",
        if env.is_empty() {
            "-".into()
        } else {
            env.join(", ")
        },
    );
    if let Ok(dir) = crate::config::config_dir() {
        line("settings in", home_to_tilde(&dir.display().to_string()));
    }
    if let Some(dir) = crate::log::dir() {
        line("logs in", home_to_tilde(&dir.display().to_string()));
    }
    match crate::config::load_auth() {
        Ok(auth) => {
            let kind = if auth.oauth_client.is_some() || auth.platform == "ios" {
                "app (renews itself)"
            } else {
                "web"
            };
            line("login", kind.to_string());
            let age = crate::config::now_unix().saturating_sub(auth.logged_in_at);
            line("logged in", format!("{} h ago", age / 3600));
            line(
                "oauth client",
                if auth.oauth_client.is_some() {
                    "stored".into()
                } else {
                    "-".into()
                },
            );
            match crate::cache::load(&auth.msisdn) {
                Some(cache) => {
                    let now_ms = crate::config::now_unix() * 1000;
                    line(
                        "file cache",
                        format!(
                            "{} files, {} pending, listed {} h ago, updated {} min ago",
                            cache.media.len(),
                            cache.pending.len(),
                            crate::config::now_unix().saturating_sub(cache.listed_at) / 3600,
                            now_ms.saturating_sub(cache.cursor) / 60_000
                        ),
                    );
                }
                None => line("file cache", "none".into()),
            }
        }
        Err(e) => line("login", format!("none ({})", e.code())),
    }
    format!(
        "o2cloud debug report: look it over before attaching it\n\n{}",
        s
    )
}

/// The operating system's version, as far as it says.
fn os_version() -> String {
    let (cmd, args): (&str, &[&str]) = match std::env::consts::OS {
        "macos" => ("sw_vers", &["-productVersion"]),
        "windows" => ("cmd", &["/C", "ver"]),
        _ => ("uname", &["-sr"]),
    };
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "-".into())
}

/// The newest `n` files in `dir` whose names start with `prefix`.
fn newest(dir: &Path, prefix: &str, n: usize) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| name_of(p).starts_with(prefix))
        .collect();
    files.sort();
    files.into_iter().rev().take(n).collect()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The home folder as `~`, so the user's account name doesn't show.
fn home_to_tilde(text: &str) -> String {
    match dirs::home_dir().map(|h| h.display().to_string()) {
        Some(home) if home.len() > 1 => text.replace(&home, "~"),
        _ => text.to_string(),
    }
}

/// A run log (JSON lines) with every name replaced: the values of
/// `NAME_FIELDS`, the command's arguments, and wherever those values
/// appear in the messages.
fn pseudonymise_log(text: &str, salt: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let Ok(mut record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let mut names: Vec<(String, String)> = Vec::new();
        if let Some(fields) = record.get_mut("fields") {
            pseudonymise_fields(fields, salt, &mut names);
        }
        if let Some(serde_json::Value::String(msg)) = record.get_mut("msg") {
            names.sort_by_key(|(original, _)| std::cmp::Reverse(original.len()));
            for (original, stand_in) in &names {
                if original.len() > 2 {
                    *msg = msg.replace(original.as_str(), stand_in);
                }
            }
            *msg = home_to_tilde(msg);
        }
        out.push_str(&record.to_string());
        out.push('\n');
    }
    out
}

fn pseudonymise_fields(
    value: &mut serde_json::Value,
    salt: &str,
    names: &mut Vec<(String, String)>,
) {
    let serde_json::Value::Object(map) = value else {
        return;
    };
    for (key, v) in map.iter_mut() {
        match v {
            serde_json::Value::String(s) if NAME_FIELDS.contains(&key.as_str()) => {
                let stand_in = crate::redact::pseudonym(&home_to_tilde(s), salt);
                names.push((s.clone(), stand_in.clone()));
                *s = stand_in;
            }
            // The arguments: the command and options stay, the rest
            // (paths, names) don't.
            serde_json::Value::Array(args) if key == "args" => {
                let mut first = true;
                for arg in args.iter_mut() {
                    if let serde_json::Value::String(a) = arg {
                        let keep = a.starts_with('-')
                            || a.parse::<f64>().is_ok()
                            || a.starts_with('<')
                            || std::mem::take(&mut first);
                        if !keep {
                            let stand_in = crate::redact::pseudonym(&home_to_tilde(a), salt);
                            names.push((a.clone(), stand_in.clone()));
                            *a = stand_in;
                        }
                    }
                }
            }
            serde_json::Value::Object(_) => pseudonymise_fields(v, salt, names),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pseudonymise_log() {
        let log = concat!(
            r#"{"lvl":"info","target":"run","msg":"start","fields":{"args":["upload-dir","./Fotos Ana","-j","4"]}}"#,
            "\n",
            r#"{"lvl":"debug","target":"decide","msg":"Ana.jpg: differs","fields":{"name":"Ana.jpg","size":10}}"#,
            "\n",
            r#"{"lvl":"error","target":"run","msg":"Not found: x","fields":{"code":"not_found"}}"#,
            "\n",
        );
        let out = pseudonymise_log(log, "salt");
        assert!(!out.contains("Ana"), "{out}");
        assert!(
            out.contains("upload-dir") && out.contains("\"-j\",\"4\""),
            "{out}"
        );
        assert!(out.contains(".jpg[7b]"), "{out}");
        assert!(out.contains("not_found"), "{out}");
        assert_eq!(out.lines().count(), 3);
    }
}
