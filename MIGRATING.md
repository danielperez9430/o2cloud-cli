# Migrating from 0.x to 1.0

1.0 changes a few defaults so that the CLI is safer and easier to drive from scripts and apps. Most people only notice that uploading twice no longer makes copies. If you call `o2cloud` from a script or another program, read this page: it lists every change that can break something, and what to do about it.

> Leer en español: [MIGRATING.es.md](MIGRATING.es.md)

## Summary

| Area | 0.x | 1.0 | What to do |
|---|---|---|---|
| [Exit codes](#exit-codes) | Every error exits with `1` | `2`–`7` by kind of error, `1` for anything else | Treat any non-zero code as an error, as before, or use the new codes |
| [Errors with `--json`](#errors-with---json) | Only `Error: …` on stderr | Also `{"error": {…}}` on stdout | Check the exit code before reading stdout as a result |
| [Session expired](#session-expired-and-the-login-window) | Opens the login window in the middle of any command | Opens it by itself only at a terminal; otherwise exit code `4` | Run `o2cloud login` when you get `4` |
| [Messages](#fewer-lines-on-stderr) | A line per file on stderr | A progress bar and a summary | `-v` for a line per file; `--json` for results |
| [`download-dir` and local files](#download-dir-skips-what-is-already-there) | Without `--skip-existing`, a local file already there was an error | Skipped if it has the same size; a cut download is resumed | Nothing; `--skip-existing` is still accepted |
| [A file already in the cloud](#files-already-in-the-cloud) | Uploaded again; O2 Cloud names it `name (1)` | Same size: skipped. Another size: a conflict (`--on-conflict`) | Add `--on-conflict duplicate` for the old behaviour, or handle exit code `5` |

What doesn't change:

- **JSON fields:** the fields 0.x prints with `--json` are still there, with the same meaning (`id`, `parts`, `skipped`, `action`…). 1.0 only adds fields.
- **File formats:** files uploaded in parts are still named `name.001`, `name.002`…
- **Settings and session:** they stay where they were, so no new login is needed.
- **Local files:** they are never overwritten.

## Exit codes

In 0.x every error exited with `1`. In 1.0 the code says what went wrong:

| Code | Meaning | `code` in JSON |
|---|---|---|
| `0` | Done | — |
| `1` | Anything else | `api_error`, `io`, `config`, … |
| `2` | A request it won't carry out as given: bad arguments, an ambiguous name, a change O2 Cloud doesn't allow | `invalid`, `ambiguous` |
| `3` | Not found | `not_found` |
| `4` | Not logged in, or the session expired | `not_logged_in`, `session_expired`, `auth_failed` |
| `5` | Conflict: something with that name is already there | `conflict` |
| `6` | Some files of a run failed; the rest were done | `partial_failure` |
| `7` | The network or O2 Cloud failed | `network`, `server_error`, `bad_response` |

**What to do:**

- If your script only checks for zero vs non-zero, nothing changes.
- If it checks for `1` specifically, check for non-zero instead.
- Codes `6` and `7` are worth retrying: run the same command again; what's already there is skipped.

## Errors with `--json`

The `Error: …` line on stderr is the same as in 0.x. With `--json`, 1.0 also prints the error to stdout, as one JSON value in place of the result:

```json
{ "error": { "code": "session_expired", "message": "Session expired: …", "retryable": false, "exit_code": 4, "run": "3fa9c1d2" } }
```

`upload-dir` and `download-dir` print their result (one entry per file) before saying that some files failed. In that case the error isn't printed to stdout as well, so stdout always holds a single JSON value.

**What to do:**

- Don't take whatever JSON is on stdout as a result. Check the exit code first.
- If you look for the words of a message, match the `code` field instead: messages can be reworded, codes won't change.

## Session expired and the login window

In 0.x, when the session had expired, any command opened the login window and waited for it. An app or a script had no way to know, except by spotting `→ Opening login window` on stderr.

In 1.0, the window only opens by itself for someone at a terminal (stdin is a terminal). Otherwise the command stops with exit code `4` and `Session expired: …`. The mobile-app login still renews itself silently, as before.

**What to do:**

- On exit code `4` (or `code` `session_expired` / `not_logged_in`), run `o2cloud login` when it suits you, then the command again.
- To keep the 0.x behaviour, set `O2CLOUD_LOGIN_WINDOW=1`.

## Files already in the cloud

In 0.x, `upload`, `upload-dir` and `upload-zip` without `--skip-existing` uploaded everything, and O2 Cloud kept both copies, naming the new one `name (1)`. Running a command twice doubled the files.

In 1.0:

- **The same name and size:** skipped, always. Running a command again carries on where it stopped.
- **The same name, another size:** a conflict, settled by `--on-conflict`:
  - `ask` (the default at a terminal): shows both copies' size and date, and asks. `upload-dir` asks everything before uploading anything.
  - `fail` (the default otherwise: scripts, apps, or messages sent to a file): doesn't upload it, and ends with exit code `5`.
  - `skip`: leaves the cloud's copy alone (`--skip-existing` does the same).
  - `overwrite`: replaces the cloud's copy. The old one goes to the trash, but only once the new one is up.
  - `duplicate`: the 0.x behaviour.

With `--json` and `fail`, each conflict has everything an app needs to ask its user:

```json
{
  "local": "photos/a.jpg", "name": "a.jpg", "folderid": 123, "size": 2400000,
  "modified": "2024-06-02T10:00:00Z", "action": "conflict", "skipped": true,
  "cloud": { "id": 456, "size": 2100000, "modified": "2024-05-01T09:30:00Z" }
}
```

A file in parts has `parts` (a list of IDs) instead of `id`.

**What to do:**

- **For the 0.x behaviour,** add `--on-conflict duplicate`.
- **In an app,** don't pass `--on-conflict` the first time. On exit code `5`, show the user each `conflict` entry ("you have a.jpg, 2.4 MB, 02/06/2024; the cloud has 2.1 MB, 01/05/2024: overwrite or skip?"). Then run it again with `--on-conflict overwrite` or `--on-conflict skip`. You can run it once per file for answers that differ.
- **In a script** that relied on `--skip-existing`, nothing changes.

## `download-dir` skips what is already there

In 0.x, `download-dir` without `--skip-existing` counted a local file that was already there as a failure (and never overwrote it). In 1.0 it always skips a local file with the same size, and resumes a cut download, as `--skip-existing` did. A local file with another size is still never overwritten: it's reported and counted as failed.

**What to do:** nothing; `--skip-existing` is still accepted. A script that expected the failures should look at the `skipped` entries instead.

## Fewer lines on stderr

0.x printed a line for every file of `upload-dir` and `download-dir` (`[12/300] 📄 …`, `= … skipped`, each part…). 1.0 shows a progress bar at a terminal, or a status line a minute when stderr goes to a file, plus warnings and the summary.

**What to do:** add `-v` to get the line per file again. Don't parse stderr for results: use `--json`. `-q` leaves only warnings and errors.

## Error messages end with the run's ID

`Error: …` lines now end with ` [run 3fa9c1d2]`, the ID of the run's log (see `debug-report` in the README). If you show or compare error messages, strip it, or use the JSON error's `code` and `message` (the message there has no ID; the ID is in `run`).

## For apps: `--json-lines`

Nothing to change, but worth knowing: instead of polling the size of the file being written, or reading what the CLI has read from the source, an app can pass `--json-lines` and read progress, each file's result (with the upload's `sha256`) and retries as they happen. See the README.

## New values in existing fields

1.0 adds values to fields that 0.x already printed. A program that rejects unknown values should accept these:

| Command | Field | New values |
|---|---|---|
| `upload-dir --json` | `action` | `conflict` |
| `upload-dir --dry-run --json` | `action` | `conflict` |
| `upload`, `upload-dir`, `download`, `download-dir` with `--json` | `ids` (new) | Always a list: the file's ID, or its parts' |
| `upload --json`, `upload-zip --json` | `action` (new) | `uploaded`, `skipped`, `updated`, `differs`, `conflict` |
| `download --json` | `action` (new) | `downloaded` |
