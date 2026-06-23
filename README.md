# o2cloud-cli

<p align="center">
  <img src="assets/logo.svg" alt="o2cloud-cli" width="600">
</p>

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
# Authentication (opens WebView)
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
o2cloud download 1195003130 -o file.txt

# Delete (trash)
o2cloud rm 1195003130                # file by ID
o2cloud rm /path/to/folder -r        # recursive folder

# Session
o2cloud status
o2cloud logout
```

## Requirements

- macOS or Linux
- O2 Cloud account (Telefónica Spain)
- Linux: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev`

## How it works

O2 Cloud uses **Synchronoss OneMediaHub v31** as its backend. Authentication goes through **Telefónica Mobile Connect** (OAuth2/OpenID Connect). The CLI opens a WebView where you enter your phone number and verify via SMS. After login, tokens are stored in:

- macOS: `~/Library/Application Support/o2cloud-cli/auth.json`
- Linux: `~/.config/o2cloud-cli/auth.json`

Sessions are silently renewed when expired — no need to re-login.

## License

MIT
