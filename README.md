# o2cloud-cli

<p align="center">
  <img src="assets/logo.svg" alt="o2cloud-cli" width="600">
</p>

[![Crates.io](https://img.shields.io/crates/v/o2cloud-cli)](https://crates.io/crates/o2cloud-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)

CLI for O2 Cloud (Telefónica Spain) — manage your cloud storage from the terminal.

[Español](README.es.md)

## Installation

```bash
cargo install o2cloud-cli
```

Or build from source:

```bash
git clone https://github.com/danielperez9430/o2cloud-cli
cd o2cloud-cli
cargo build --release
./target/release/o2cloud --help
```

## Usage

```bash
# Authentication (opens a login window: enter your phone number and the SMS code there)
o2cloud login

# List files
o2cloud ls                  # root
o2cloud ls /DMHAIR          # folder by path
o2cloud ls -t               # tree view
o2cloud ls -a               # all files (flat)

# Search
o2cloud find "query"        # case-insensitive search

# Upload
o2cloud upload file.txt
o2cloud upload-dir ./my-folder       # recursive
o2cloud upload-zip ./my-folder       # zip + upload

# Download
o2cloud download 1195003130          # saved under its cloud name in the current dir
o2cloud download 1195003130 -o file.txt

# Delete (trash)
o2cloud rm 1195003130                # file by ID
o2cloud rm /path/to/file.txt         # file by path
o2cloud rm /path/to/folder -r        # recursive folder (prompts for confirmation)
o2cloud rm /path/to/folder -r -y     # recursive folder, skip confirmation

# Session
o2cloud status
o2cloud logout
```

### Notes

- `upload-dir` recurses into all subfolders. Hidden files and folders
  (`.git`, `.DS_Store`, …) are skipped because O2 Cloud rejects them.
- `upload-dir` and `upload-zip` follow symlinks to files, but **skip
  symlinks to folders** (with a warning), so a link pointing back to a
  parent folder can't cause an endless loop.
- `download` never overwrites an existing local file. Without `-o`, only
  the file name is used (any directory part sent by the server is ignored).

## Requirements

- macOS, Linux or Windows
- O2 Cloud account (Telefónica Spain)
- Linux: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev`
- Windows: the [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalled on Windows 11 and up-to-date Windows 10)

## How it works

O2 Cloud uses **Synchronoss OneMediaHub v31** as its backend. Authentication uses the same flow as the O2 Cloud website: Telefónica's OAuth2/OpenID Connect login, with PKCE handled by the O2 Cloud server (no client secret involved). The CLI opens a login window where you enter your phone number and verify via SMS. After login, the session is stored in:

- macOS: `~/Library/Application Support/o2cli/auth.json`
- Linux: `~/.config/o2cli/auth.json`
- Windows: `%APPDATA%\o2cli\auth.json`

On macOS/Linux the file is only readable by your user (`0600`).

A session lasts about 24 hours. When it expires, the next command opens the login window again.

## License

MIT
