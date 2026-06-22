# Changelog

All notable changes to o2cloud-cli will be documented in this file.

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
