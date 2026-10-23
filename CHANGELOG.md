# Changelog

All notable changes to o2cloud-cli will be documented in this file.

## [Unreleased]

### Added
- `login --jsessionid <value> --validationkey <value>` logs in with the
  session cookies of a logged-in browser, for machines where the login
  window can't open (headless, some Wayland setups) (#4, thanks
  @juliokarl). The values can also come from `O2CLOUD_JSESSIONID` and
  `O2CLOUD_VALIDATIONKEY`, are checked against O2 Cloud before they're
  saved, and stray whitespace from copying them is ignored.
- When a session expires, the message before the login window opens
  points to the cookie login.

### Security
- Updated `ctutils` 0.4.3 → 0.4.5 (used by `zip`) to clear
  RUSTSEC-2026-0385.

## [0.3.0] — 2026-10-23

**Upgrading:** run `o2cloud login` again. The phone number and SMS code
are now entered in the login window (the old login no longer works on
O2's side), and sessions last about 24 hours.

### Added
- `share <file|folder>` creates a public link and prints its URL (the
  root folder is refused); `shares` lists shared folders with their link
  ID, uses and URL; `unshare <folder|link-id>` removes a folder's link.
- `mv <file> <folder>` moves a file and `rename <file> <name>` renames
  it; both refuse to overwrite a file with the same name.
- `status` checks the session against the server ("active" / "expired")
  and shows when it was created; it no longer prints part of the
  validation key, which is a session credential.
- Folders aren't supported yet by `mv`/`rename`, nor files by `unshare`:
  the CLI says so instead of guessing at the requests.

### Fixed
- **`login` was broken** (#2): O2 Cloud retired the Mobile Connect login
  the CLI used, so `mobileconnect?action=start` failed with
  `PAPI-0000 Unknown exception` for every number. The CLI now uses the
  same OAuth2 login as the O2 Cloud website (`/sapi/oauth/pkce/authorize`
  → Telefónica login → `/sapi/login/oauth`), with PKCE handled by the
  server. The phone number and SMS code are now entered in the login
  window; `--phone` is accepted but ignored.
- `upload-zip` uploaded a corrupt archive: the zip's central directory was
  only written when the writer was dropped, after the upload had already
  happened. The archive is now finalized before uploading.
- `upload-zip .` (or `..`) panicked; the archive is now named after the
  resolved directory.
- `upload-zip` left the temporary zip behind when the upload failed, and
  built it at a predictable path in the shared temp dir. It now uses a
  fresh private (0700) temp directory that is always removed.
- `upload-zip` no longer reads each file fully into memory while zipping.
- `download` without `-o` used the server-supplied media name as a local
  path, so a name like `../../.bashrc` could write outside the current
  directory. Only the final file-name component is used now.
- `download` could overwrite a file created between the existence check
  and the download; the file is now created with `create_new`.
- `ls` and `ls -t` never showed files the server returns without a folder
  (or with an unknown one), e.g. a large upload still being processed;
  `find` showed them under `/`. They are now listed in the root too.
- `upload-dir ./my-folder` uploaded the folder's contents straight into
  the target instead of into a `my-folder/` folder (only subfolders were
  created). It now creates `my-folder/` in the target, or reuses it if a
  folder with that name already exists.
- `upload-dir` and `upload-zip` recursed forever on a symlink pointing
  back to an ancestor directory. Symlinked directories are now skipped
  (with a warning); symlinked files are still followed.

- `rm` could trash the whole account: `rm / -r` resolved to the root
  folder. Deleting the root folder is now refused.
- `rm <name>` deleted the first file with that name found anywhere in
  the account. If several files share the name it now errors and lists
  their IDs and folders instead of guessing.
- `rm` on a non-empty folder without `-r` printed a hint but exited 0;
  it now fails with a non-zero exit code.
- `rm <id>` with an ID that matched no file or folder still sent a delete
  request; it now reports "not found".
- Numeric names like `2024` couldn't be used as folder names in `ls`/`rm`
  (always parsed as an ID). If no folder has that ID, the name is used.
- Being offline (or any network error) during the session check was
  treated as an expired session and opened the login window; the network
  error is now reported instead.
- Uploads now use the website's single multipart request
  (`/sapi/upload?action=save`), and file deletes its batch endpoint
  (`/sapi/media/file?action=delete&softdelete=true`), so `rm -r` sends one
  request per 1000 files instead of one per file.
- Errors the server returns inside a 200 response (`{"error": …}`) are
  now reported with the server's message and code.
- `login` without a graphical session (no `DISPLAY`/`WAYLAND_DISPLAY`)
  panicked; it now reports a clear error. Same for a missing config
  directory.

### Changed
- Refusals and argument errors (e.g. "refusing to delete the root
  folder", "already has a file named …") no longer carry a misleading
  "I/O error:" prefix. `shares` puts the folder in the last column so
  long paths don't break the table.
- README (EN/ES) rewritten: command reference, file/folder naming rules,
  example output, and placeholder IDs and names instead of real ones.
- `download` accepts a file path or a unique file name as well as an ID
  (`o2cloud download /docs/a.txt`), resolved like `rm`.
- `rm` accepts file paths (`o2cloud rm /docs/a.txt`), and the recursive
  delete prompt now shows which folder will be moved to the trash (it
  previously said "permanently delete", but deletes are soft).
- Sessions from the new login last ~24 h and are not renewed silently
  (the server no longer hands the client a renewable token); the login
  window opens again when they expire. Existing sessions from the old
  login keep their silent renewal. When silent renewal fails, the reason
  is printed.
- Directory walking, zipping, and file metadata reads now run off the
  async runtime (`spawn_blocking` / `tokio::fs`), and `upload-dir` reads
  each file's metadata once instead of twice.
- More accurate upload content types for video (`mov`, `avi`, `mkv`,
  `webm`), audio (`wav`, `flac`, `aac`, `ogg`, `m4a`), `zip`/`gz` and
  `heic`; previously all video was sent as `video/mp4` and all audio as
  `audio/mpeg`.

### Security
- `auth.json` is created with mode 0600 from the start (it was previously
  written with default permissions and chmod'ed afterwards), and the config
  directory is restricted to 0700.
- Updated dependencies to clear RUSTSEC advisories: `zip` 2 → 9, plus
  lockfile updates for `rustls`, `rustls-webpki`, `webpki-roots`,
  `quinn-proto` and `quick-xml`.
- CI: the workflow token is now read-only except for the release job, and
  `cargo audit` runs on every push/PR.

## [0.2.0] — 2026-07-14

### Added
- `rm --recursive` now prints how many files/subfolders it's about to
  destroy and asks for confirmation before deleting anything. Pass
  `-y`/`--yes` to skip the prompt (required when stdin isn't a TTY,
  e.g. in scripts).
- HTTP request timeouts: a 15s connect timeout on the client, plus a
  60s per-request timeout on metadata/listing/login calls (uploads and
  downloads of file bytes are intentionally exempt).

### Changed
- Uploads and downloads are now streamed to/from disk instead of being
  buffered fully in memory, so peak memory use no longer scales with
  file size.
- Errors are no longer all reported as "Authentication failed: ...".
  `O2Error` now distinguishes API failures, not-found resources,
  malformed responses, and local I/O errors from genuine login/session
  failures.
- `main.rs`'s per-command error handling was collapsed from nine
  repeated match/eprintln/exit blocks into a single `run()` function
  using `?`.

### Fixed
- Error-body previews in `api.rs` could panic when truncated mid multi-byte
  UTF-8 character (server responds in Spanish).
- User-facing messages referenced the command as `o2cli`; the binary is
  actually `o2cloud`.
- `upload-dir`/`upload-zip` could panic on non-UTF-8 file names.
- ZIP entries used the host OS path separator instead of `/`, producing
  broken archives when zipping on Windows.
- `upload-dir` now skips hidden **directories** as well as hidden files
  (previously `.git/`, `.venv/`, etc. had their contents uploaded).
- `upload-dir`'s final success count no longer reports files that failed
  mid-upload as successful; the command now exits non-zero if any file
  failed.
- `rm --recursive` fetched the full media list at every level of folder
  recursion; it's now fetched once per invocation.
- `ls`/`find` on a folder ID that doesn't exist now errors instead of
  silently printing an empty folder.
- `download` without `-o` now saves the file under its real name (was
  `<id>.downloaded`); it also refuses to overwrite an existing local file.

### Removed
- The local file cache (`~/.config/o2cli/file_cache.json`) has been
  removed — it was written on every upload but never read anywhere. Any
  existing cache file on disk is harmless and can be deleted manually.

## [0.1.0] — 2026-06-22

### Added
- `login` — WebView-based OAuth2 authentication via Telefónica Mobile Connect
- `ls` — list files and folders with path navigation (`ls /path`, `ls <id>`)
- `ls -t` — hierarchical tree view
- `ls -a` — flat list of all files across folders
- `find` — case-insensitive search by file or folder name
- `upload` — upload a single file
- `upload-dir` — recursive directory upload with folder auto-creation and progress counter
- `upload-zip` — zip a directory and upload as a single file
- `download` — download a file by media ID
- `rm` — soft-delete files and folders (move to trash) with `-r` recursive flag
- `status` — show login status
- `logout` — clear stored credentials
- Silent session renewal when `JSESSIONID` expires via stored `pat` token
- Automatic re-login fallback when silent renewal fails
- Pagination for accounts with >1000 items (`limit`/`offset` query params)
- Dotfile filtering in `upload-dir` (O2 Cloud blocks `.` prefixed files)
- Token persistence in OS config directory
- CI pipeline: test → build → release → publish on tag
- Release binaries: macOS (ARM + Intel), Linux x86_64, Windows x86_64
- Publish to crates.io on tag via `CARGO_REGISTRY_TOKEN`
