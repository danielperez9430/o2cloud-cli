# Changelog

All notable changes to o2cloud-cli will be documented in this file.

## [Unreleased]

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
