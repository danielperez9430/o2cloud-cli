# o2cloud-cli

<p align="center">
  <img src="assets/logo.svg" alt="o2cloud-cli" width="600">
</p>

<p align="center">
  <a href="https://crates.io/crates/o2cloud-cli"><img src="https://img.shields.io/crates/v/o2cloud-cli" alt="Crates.io"></a>
  <a href="https://github.com/danielperez9430/o2cloud-cli/actions/workflows/release.yml"><img src="https://github.com/danielperez9430/o2cloud-cli/actions/workflows/release.yml/badge.svg" alt="CI"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT"></a>
</p>

<p align="center">
  <b>Manage your O2 Cloud (Telefónica Spain) storage from the terminal.</b><br>
  <a href="README.es.md">Español</a>
</p>

---

## ✨ Features

- 🔐 **Login** with your phone number and SMS code, in the same login window the O2 Cloud website uses
- 📂 **Browse** folders as a list or a tree, and **search** by name
- ⬆️ **Upload** single files, whole folders, or a folder as a `.zip`
- ⬇️ **Download** by ID, path or file name
- 🔗 **Share** files and folders with a public link, and remove the link again
- ✏️ **Move** and **rename** files
- 🗑️ **Delete** to the trash, with safety checks (confirmation for folders, the root folder can't be deleted, ambiguous names are never guessed)

## 📦 Installation

With [Rust](https://rustup.rs) installed:

```bash
cargo install o2cloud-cli
```

Pre-built binaries for macOS, Linux and Windows are attached to each [GitHub release](https://github.com/danielperez9430/o2cloud-cli/releases).

<details>
<summary>🛠️ Build from source</summary>

```bash
git clone https://github.com/danielperez9430/o2cloud-cli
cd o2cloud-cli
cargo build --release
./target/release/o2cloud --help
```

</details>

### 🧰 Requirements

| Platform | Needs |
|---|---|
| macOS | Nothing extra |
| Linux | `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev` |
| Windows | The [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalled on Windows 11 and up-to-date Windows 10) |

You also need an O2 Cloud account (an O2 Spain mobile line).

## 🚀 Quick start

```console
$ o2cloud login
→ Opening login window (enter your phone number there)...
✓ Login successful — tokens saved
Logged in as +34 600000000

$ o2cloud ls
📁 /

  12345678  📁 Photos/
  12345679  📁 Documents/

  1234567890    1.2 MB  notes.pdf

$ o2cloud upload-dir ./holidays --folder 12345678
→ Uploading ./holidays (24.0 MB) — 12 files, 1 dirs...
  📁 holidays/ (created id:12345690)
  ...
✓ Uploaded 12 files to ./holidays
```

## 📖 Commands

| Command | What it does |
|---|---|
| 🔐 `login` | Log in (opens a login window) |
| ℹ️ `status` | Show the account and whether the session is still active |
| 🚪 `logout` | Remove the stored session |
| 📂 `ls [folder]` | List a folder (`-t` tree view, `-a` all files) |
| 🔍 `find <text>` | Search files and folders by name |
| ⬆️ `upload <file>` | Upload a file (`--folder <id>` to choose where) |
| 📁 `upload-dir <dir>` | Upload a folder and everything in it |
| 🗜️ `upload-zip <dir>` | Zip a folder and upload the `.zip` |
| ⬇️ `download <file>` | Download a file (`-o` to choose the local path) |
| 🗑️ `rm <file or folder>` | Move to the trash (`-r` for folders, `-y` to skip confirmation) |
| 🔗 `share <file or folder>` | Create a public link and print its URL |
| 📋 `shares` | List shared folders |
| 🚫 `unshare <folder>` | Remove a folder's public link |
| 📦 `mv <file> <folder>` | Move a file to another folder |
| ✏️ `rename <file> <name>` | Rename a file |

Run `o2cloud <command> --help` for all the options.

### 🎯 Naming files and folders

Wherever a command takes a file or folder, you can use any of these:

| Form | Example | Meaning |
|---|---|---|
| ID | `1234567890` | As shown by `ls` and `find` |
| Path | `/Documents/notes.pdf` | Starts with `/`: an exact path from the root |
| Name | `notes.pdf` | No `/`: searched in the whole account; must be unique |

If a name matches several files, nothing happens and the CLI lists the matches so you can pick one:

```console
$ o2cloud download notes.pdf
Error: 'notes.pdf' is ambiguous — 2 files have that name:
  1234567890    /notes.pdf
  1234567891    /Documents/notes.pdf
Use the ID or the full path.
```

## 💡 Examples

<details open>
<summary><b>📂 Browse and search</b></summary>

```bash
o2cloud ls                     # root folder
o2cloud ls /Photos/2024        # a folder by path
o2cloud ls 12345678            # a folder by ID
o2cloud ls -t                  # tree view
o2cloud ls -a                  # every file, flat
o2cloud find invoice           # case-insensitive search
```

</details>

<details open>
<summary><b>🔄 Upload and download</b></summary>

```bash
o2cloud upload ./report.pdf                      # to the root folder
o2cloud upload ./report.pdf --folder 12345679    # to a folder (by ID)
o2cloud upload-dir ./holidays                    # creates holidays/ and uploads into it
o2cloud upload-zip ./project                     # uploads project.zip

o2cloud download /Documents/report.pdf           # saved as ./report.pdf
o2cloud download 1234567890 -o ~/Desktop/copy.pdf
```

</details>

<details open>
<summary><b>🔗 Share</b></summary>

```console
$ o2cloud share /Photos/2024
✓ Public link created for /Photos/2024/ — anyone with it can access it:
https://cloud.o2online.es/share/f/EXAMPLE-TOKEN
  (link 100001; remove it with `o2cloud unshare 100001`)

$ o2cloud shares
LINK ID     USES  URL                                            FOLDER
100001         3  https://cloud.o2online.es/share/f/EXAMPLE-TOKEN  /Photos/2024/

$ o2cloud unshare /Photos/2024
✓ /Photos/2024/ is no longer shared
```

The URL is printed alone on standard output, so scripts can capture it: `url=$(o2cloud share /notes.pdf)`.

</details>

<details open>
<summary><b>🗂️ Move, rename and delete</b></summary>

```bash
o2cloud mv /notes.pdf /Documents
o2cloud rename /Documents/notes.pdf notes-2024.pdf

o2cloud rm /Documents/notes-2024.pdf    # a file
o2cloud rm /Photos/old -r               # a folder and its contents (asks first)
o2cloud rm /Photos/old -r -y            # same, without asking (for scripts)
```

</details>

## ⚠️ Good to know

> [!NOTE]
> **Sessions last about 24 hours** and aren't renewed automatically. When one expires, the next command opens the login window again (or, without one, log in again with [browser cookies](#️-logging-in-without-a-login-window)). `o2cloud status` tells you whether yours is still active.

> [!WARNING]
> **Shared links are public**: anyone with the URL can open the file or folder. Sharing the root folder is refused, since it would publish the whole account.

- **Deletes go to the trash**, so they can be restored from the O2 Cloud website.
- **`download` never overwrites** an existing local file. Without `-o`, it only uses the file's name, never a directory sent by the server.
- **`upload-dir` skips hidden files and folders** (`.git`, `.DS_Store`…), because O2 Cloud rejects them. `upload-zip` includes them inside the zip.
- **Symlinks:** links to files are followed; links to folders are skipped with a warning, so a link back to a parent folder can't cause an endless loop.
- **Not supported yet:** unsharing a file, and moving or renaming folders. Use the website for those for now.

## 🖥️ Logging in without a login window

On a headless machine, or a Linux desktop where the login window can't open (for example some Wayland setups), you can log in with the session cookies of a browser instead:

1. Log in at [cloud.o2online.es](https://cloud.o2online.es) in any browser.
2. Open the developer tools (F12) → **Application** (Chrome) or **Storage** (Firefox) → **Cookies**.
3. Copy the values of `JSESSIONID` and `validationkey`, and pass them to `login`:

```bash
o2cloud login --jsessionid "<JSESSIONID>" --validationkey "<validationkey>"
```

To keep them out of your shell history, use environment variables instead:

```bash
export O2CLOUD_JSESSIONID="<JSESSIONID>" O2CLOUD_VALIDATIONKEY="<validationkey>"
o2cloud login
```

The cookies are checked before they are saved. Like any session, this one lasts about 24 hours; after that, copy fresh cookies and log in again.

## ⚙️ How it works

O2 Cloud runs on **Synchronoss OneMediaHub**. `o2cloud` talks to the same API as the O2 Cloud website, and logs in the same way: Telefónica's OAuth2 / OpenID Connect login, with PKCE handled by the O2 Cloud server (no client secret involved). You type your phone number and SMS code in the login window, never in the terminal.

The session is stored in:

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/o2cli/auth.json` |
| Linux | `~/.config/o2cli/auth.json` |
| Windows | `%APPDATA%\o2cli\auth.json` |

On macOS and Linux the file is readable only by your user (`0600`). `o2cloud logout` deletes it.

## 📄 License

[MIT](./LICENSE)
