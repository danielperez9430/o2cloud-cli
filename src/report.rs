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
    // Random, so a report's stand-ins can't be worked back from its date.
    let salt = {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(crate::config::now_unix());
        format!("{:016x}", h.finish())
    };
    let summary = summary();
    let mut raw: Vec<(String, String)> = Vec::new();
    if let Some(dir) = crate::log::dir() {
        for path in newest(&dir, "run-", RUNS + 1)
            .into_iter()
            // Not this run's own log, which only says it made a report.
            .filter(|p| Some(p) != crate::log::path())
            .take(RUNS)
        {
            raw.push((
                name_of(&path),
                std::fs::read_to_string(&path).unwrap_or_default(),
            ));
        }
        for path in newest(&dir, "crash-", 2) {
            raw.push((
                name_of(&path),
                std::fs::read_to_string(&path).unwrap_or_default(),
            ));
        }
    }
    let files: Vec<(String, String)> = if include_names {
        raw.into_iter()
            .map(|(name, text)| (name, home_to_tilde(&text)))
            .collect()
    } else {
        // Every name seen anywhere, replaced everywhere.
        let names = Names::collect(raw.iter().map(|(_, t)| t.as_str()), &salt);
        raw.into_iter()
            .map(|(name, text)| {
                let text = if name.starts_with("run-") {
                    names.log(&text)
                } else {
                    names.text(&text)
                };
                (name, text)
            })
            .collect()
    };

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
        // Also as JSON writes it, with `\` doubled (Windows paths).
        Some(home) if home.len() > 1 => text
            .replace(&home, "~")
            .replace(&home.replace('\\', "\\\\"), "~"),
        _ => text.to_string(),
    }
}

/// The names of files and folders in a set of logs, each with its
/// stand-in: the values of `NAME_FIELDS`, the command's arguments (and
/// `--option=value` values), and each part of those paths. They're
/// replaced longest first in every string of every record (messages and
/// errors too), and in crash reports.
struct Names {
    pairs: Vec<(String, String)>,
}

impl Names {
    fn collect<'a>(texts: impl Iterator<Item = &'a str>, salt: &str) -> Names {
        let mut found: std::collections::BTreeSet<String> = Default::default();
        for text in texts {
            for line in text.lines() {
                if let Ok(record) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(fields) = record.get("fields") {
                        names_in(fields, &mut found);
                    }
                } else if let Some(args) = line.strip_prefix("args: ") {
                    // A crash report's command line.
                    for arg in args.split_whitespace().skip(1) {
                        add_arg(arg, &mut found);
                    }
                }
            }
        }
        // Each part of a path, so a message naming only that part is
        // caught too.
        let parts: Vec<String> = found
            .iter()
            .flat_map(|p| p.split(['/', '\\']).map(str::to_string).collect::<Vec<_>>())
            .collect();
        found.extend(parts);
        let mut pairs: Vec<(String, String)> = found
            .into_iter()
            .filter(|n| n.chars().count() > 2 && n != ".." && n.parse::<f64>().is_err())
            .map(|n| {
                let stand_in = crate::redact::pseudonym(&home_to_tilde(&n), salt);
                (n, stand_in)
            })
            .collect();
        pairs.sort_by_key(|(original, _)| std::cmp::Reverse(original.len()));
        Names { pairs }
    }

    /// `text` with every name replaced (and the home folder as `~`).
    fn text(&self, text: &str) -> String {
        let mut out = home_to_tilde(text);
        for (original, stand_in) in &self.pairs {
            if out.contains(original.as_str()) {
                out = out.replace(original.as_str(), stand_in);
            }
            // As JSON writes it (`\\` for `\`).
            let escaped = original.replace('\\', "\\\\");
            if escaped != *original && out.contains(&escaped) {
                out = out.replace(&escaped, stand_in);
            }
        }
        out
    }

    /// A run log (JSON lines) with every name replaced in every string.
    fn log(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            let Ok(mut record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            self.value(&mut record);
            out.push_str(&record.to_string());
            out.push('\n');
        }
        out
    }

    fn value(&self, v: &mut serde_json::Value) {
        match v {
            serde_json::Value::String(s) => *s = self.text(s),
            serde_json::Value::Array(a) => a.iter_mut().for_each(|v| self.value(v)),
            serde_json::Value::Object(m) => m.values_mut().for_each(|v| self.value(v)),
            _ => {}
        }
    }
}

/// The names in a record's `fields`.
fn names_in(value: &serde_json::Value, found: &mut std::collections::BTreeSet<String>) {
    let serde_json::Value::Object(map) = value else {
        return;
    };
    for (key, v) in map {
        match v {
            serde_json::Value::String(s) if NAME_FIELDS.contains(&key.as_str()) => {
                found.insert(s.clone());
            }
            serde_json::Value::Array(args) if key == "args" => {
                // The first is the command; the rest that aren't options
                // or numbers are paths and names.
                for arg in args.iter().skip(1).filter_map(|a| a.as_str()) {
                    add_arg(arg, found);
                }
            }
            serde_json::Value::Object(_) => names_in(v, found),
            _ => {}
        }
    }
}

/// A command-line argument's name, if it holds one: a value, or the value
/// of `--option=value`; not options, numbers or redacted secrets.
fn add_arg(arg: &str, found: &mut std::collections::BTreeSet<String>) {
    let value = match arg.split_once('=') {
        Some((flag, value)) if flag.starts_with('-') => value,
        _ if arg.starts_with('-') => return,
        _ => arg,
    };
    if !value.is_empty() && !value.starts_with('<') && value.parse::<f64>().is_err() {
        found.insert(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_names_replaced_everywhere() {
        let log = concat!(
            r#"{"lvl":"info","target":"run","msg":"start","fields":{"args":["upload-dir","./Fotos Ana","-j","4","--output=/x/Nóminas Ana"]}}"#,
            "\n",
            r#"{"lvl":"debug","target":"decide","msg":"Boda.jpg: differs","fields":{"local":"2019.Boda Luis/Boda.jpg","size":10}}"#,
            "\n",
            r#"{"lvl":"error","target":"run","msg":"Not found: /Fotos Ana/2019.Boda Luis","fields":{"code":"not_found"}}"#,
            "\n",
        );
        let crash =
            "o2cloud 1.0.0\nargs: upload-dir ./Fotos Ana --phone <redacted>\n\npanicked at x";
        let names = Names::collect([log, crash].into_iter(), "salt");
        let out = names.log(log) + &names.text(crash);
        for name in ["Ana", "Boda", "Luis", "Nóminas"] {
            assert!(!out.contains(name), "{name} in {out}");
        }
        assert!(
            out.contains("upload-dir") && out.contains("\"-j\",\"4\""),
            "{out}"
        );
        assert!(
            out.contains("not_found") && out.contains("Not found:"),
            "{out}"
        );
        assert_eq!(names.log(log).lines().count(), 3);
    }
}
