# Changelog

All notable changes to o2cloud-cli will be documented in this file.

## [1.2.2] — 2026-11-26

### Fixed
- `--on-conflict overwrite` (and `upload-dir --update`) could fail when
  the cloud copy was uploaded moments before and O2 Cloud was still
  checking it (`MED-1017`), and leave the new version as `name (1)`
  (#55). Since 1.2.0 an upload no longer waits for O2 Cloud to list the
  previous one, so the replacement reached it sooner; renaming and
  deleting it gave up after about 12 s, and O2 Cloud can take longer.
  Each step of a replacement, and taking it back if something fails, now
  waits up to 2 minutes for O2 Cloud, saying so (and with a `waiting`
  event in `--json-lines`).

## [1.2.1] — 2026-11-26

### Fixed
- Files and folders with emoji in their names (😀, 📷, flags…) were
  uploaded again on every `upload-dir --update` / `--skip-existing`, and
  O2 Cloud kept each copy as `name (1)`, `name (2)`… O2 Cloud drops every
  character outside Unicode's Basic Multilingual Plane from names (`foto
  😀.jpg` becomes `foto .jpg`; checked live), so the local name never
  matched the cloud's. Names are now compared as O2 Cloud stores them,
  the upload says how a name will be kept, and a name left with nothing
  before its extension (`😀.jpg`) is reported instead of failing.
- Right after an upload, `ls` showed such a file under the name sent
  rather than the one O2 Cloud keeps.

## [1.2.0] — 2026-11-26

### Added
- `o2cloud media photos|videos|music|documents`: the account's files of
  one kind, wherever they are, as O2 Cloud's website groups them
  (Música, Documentos…). `--json` gives each file's `path`, `type`,
  `thumbnail` and `preview`, `location`, a song's `duration`, and a
  signed `url` that opens or streams it (with ranges) without the session.
- `labels --json`: each label's `thumbnail` (its cover's) and, for
  places, a `location` (`{"lat", "lon"}`, where its cover was taken, from
  the photo's GPS data). `labels <label> --json` gives each file its
  `type`, `location`, `thumbnail` (about 320 px) and `preview` (about
  1440 px). Thumbnail URLs are signed: they open without the session, for
  a while, so treat them as private.

### Changed
- A file shows up in `ls` (and `media`) as soon as it's uploaded, instead
  of 10–60 s later when O2 Cloud lists it: the CLI keeps what it sent
  until the server returns it. Uploading again right after no longer
  waits for O2 Cloud to list the previous upload, and still sees it (a
  conflict, or skipped), never uploading a twin.
- Listing doesn't wait (up to 12 s) for a file O2 Cloud is still
  validating: it stays listed as uploaded.
- `labels --json`: a label's `thumbnail` and `location` come from its
  cover, or its first photo if the cover is gone, for any kind of label.

### Fixed
- `labels <label> --json` gave every file a `null` path: O2 Cloud sends no
  folder for files asked for by ID, so it now comes from the listing.
  Files with no folder (the phone app's photos) get `/name`; `null` now
  means a file not in the listing yet.
- Error messages from a response that couldn't be read no longer carry
  O2 Cloud's signed download links.

## [1.1.0] — 2026-11-26

### Added
- `o2cloud labels`: the moments ("A day like today, 1 year ago"…),
  places and people O2 Cloud groups photos into by itself, and albums,
  with how many photos each has and its date (`--kind
  moments|places|people|albums`, `--json`). People appear only once face
  recognition is turned on in O2 Cloud's website. `labels <id or name>`
  lists a label's photos and videos.
- `download-dir --label <id or name>`: downloads a label's photos and
  videos into a folder of its name, with everything `download-dir` does
  (resume, skip what's there, safe names, `--dry-run`, `--json`,
  `--json-lines`).

## [1.0.1] — 2026-11-26

### Changed
- Replacing a file (`--on-conflict overwrite`, `upload-dir --update`) takes
  about a second instead of 7–10: the old copy is renamed out of the way
  first (to `name.o2-replacing-<id>.ext`), the new one goes up under the
  real name, and then the old one goes to the trash. Before, the new one
  went up as `name (1)` and had to wait for O2 to validate it before it
  could be renamed. If anything fails, the old copy gets its name back.
  Where the temporary name would be over O2 Cloud's limit, it works as
  before.

## [1.0.0] — 2026-11-26

The first stable version: safer defaults (uploading twice never makes
copies; conflicts are asked about or reported), exit codes and JSON
errors that scripts and apps can rely on, live progress for apps
(`--json-lines`), a progress bar, and logs plus `debug-report` for bug
reports, with session keys and personal names kept out of them. See
[MIGRATING.md](MIGRATING.md) for what to change in scripts and apps.

### Added
- `upload` takes several files, and the folder to put them in as the
  last argument (`upload a.jpg b.jpg /Photos/2024`); `--folder` of
  `upload`, `upload-dir` and `upload-zip` takes a path as well as an ID.
  With `--json`, one file still gives one object, several a list;
  entries have `local`, and a single file that fails gives the JSON
  error instead of an entry.
- `rm` takes several files or folders, and goes on past one it can't
  delete (exit code 6).
- `--json-lines` for `upload`, `upload-dir`, `upload-zip`, `download` and
  `download-dir`: events as they happen, one JSON object per line
  (`start`, `progress`, `file`, `retry`, `summary`, `error`), for apps
  that want real progress. Upload `file` events have the file's
  `sha256`, computed while it's read to be sent (when all of it was
  sent in this run).
- `--json` entries of `upload`, `upload-dir`, `download` and
  `download-dir` have `ids`, always a list (one ID, or the parts'), next
  to `id` / `parts`.
- `o2cloud debug-report`: a zip to attach to bug reports, with the
  version, system, settings (no keys), session kind and age, cache
  stats and the logs of the last 3 runs, with file and folder names
  replaced by stand-ins (`--include-names` keeps them, `--stdout` prints
  it). Nothing is sent anywhere.
- A log of each run of a command that changes or transfers something
  (JSON lines, last 10 runs, 5 MB each; a log written to in the last
  hour is never removed) in the cache folder, removed by `logout`;
  `O2CLOUD_LOG_FILE=0` turns it off. Failed files are logged with their
  error. Errors
  end with the run's ID (`[run 3fa9c1d2]`), also in the JSON error as
  `run`.
- `--debug` (and `O2CLOUD_LOG=debug|info|warn|error`): each request,
  retry, the session and cache, and the decision for each file of
  `upload-dir`, on stderr (as `[debug] …` lines) and in the run log.
- A crash leaves a report next to the logs and says where.
- Session keys, tokens, the phone number, OAuth client and signed links
  are removed from everything shown or logged, error messages included;
  so are the values of `--jsessionid`, `--validationkey` and the OAuth
  options in the logged command line.

### Changed (breaking)
- `download-dir` always skips files already there with the same size
  (and resumes cut downloads); `--skip-existing` is still accepted.
- `upload --json` entries always have `action` (`uploaded`, `skipped`,
  `updated`, `differs`, `conflict`), and `download --json` has
  `"action": "downloaded"`.
- Long transfers show a progress bar (files, bytes, speed, time left) on
  one line at a terminal, or a status line a minute in a log file,
  instead of a line per file. `-v` / `--verbose` brings back a line for
  each file (uploaded, skipped, each part…), and `-q` / `--quiet` shows
  only warnings and errors. Warnings (failures, conflicts, files left
  out) always show.
- Exit codes say what went wrong: 2 a request it won't carry out as
  given, 3 not found, 4 not logged in or session expired, 5 conflict
  (something with that name is already there), 6 some files of a run
  failed, 7 the network or O2 Cloud failed, 1 anything else. Before,
  every error exited with 1.
- With `--json`, an error is also printed to stdout as
  `{"error": {"code", "message", "retryable", "exit_code"}}`, with a
  stable `code` (unless the command's result was already printed).
- When the session has expired, the login window only opens by itself at
  a terminal; scripts and apps get exit code 4 ("Session expired") and
  log in with `o2cloud login`. `O2CLOUD_LOGIN_WINDOW=1` opens it anyway.
- Not having logged in is its own error ("Not logged in: run `o2cloud
  login`", code `not_logged_in`).
- `download-dir` that stops because the session expired says so (exit
  4), and one that stops because the disk is full says that.
- Uploading never makes a second copy by default. `upload`, `upload-dir`
  and `upload-zip` always skip what's already there with the same name
  and size, and a file there with another size is a conflict, settled by
  the new `--on-conflict ask|skip|overwrite|fail|duplicate`: `ask` at a
  terminal (both copies' size and date, then overwrite / skip / all /
  quit; `upload-dir` asks before uploading anything), `fail` otherwise
  (not uploaded, exit code 5, and `--json` entries with
  `"action": "conflict"` and the cloud copy's `id`/`parts`, `size` and
  `modified`). `duplicate` is the old behaviour (O2 Cloud names the new
  one `name (1)`). `--skip-existing` stays, as `--on-conflict skip`.
  `overwrite` also replaces a file of the same size that is newer
  locally.
- `upload --json` of a file replaced with `--on-conflict overwrite` has
  `"action": "updated"` and `replaced`.

## [0.15.4] — 2026-11-26

### Security
- A failed request (a timeout, a cut connection) printed its full URL in
  the error message, and with `O2CLOUD_TRACE=1` in the trace: for SAPI
  requests that includes the session's `validationkey`, and for
  downloads a signed link to the file. Those messages end up in logs
  people attach to bug reports. Errors no longer carry the URL. If you
  posted such a log, log out and in again (`o2cloud logout`, then
  `o2cloud login`) to end that session.

### Changed
- The message when a session expires in the middle of a command now
  starts with "Session expired", the words apps look for (the Tauri app
  didn't notice it and didn't ask to log in again).

## [0.15.3] — 2026-11-26

### Fixed
- `upload-dir --skip-existing` and `--update` on large accounts: finding
  each file's parts went through every file in the cloud, so a folder of
  hundreds of thousands of files took minutes before uploading anything.
  The parts are now indexed by name.
- `--skip-existing` never leaves two versions of a file: a file that
  crossed the part size (whole in the cloud, in parts now, or the other
  way round) is left alone as `differs` in `upload-dir` (before, both
  versions stayed), and `--update` replaces it.
- A local `name.001` next to `name` is that file, never a part of
  `name`: `--update` no longer moves it to the trash as an old version.
- `upload --skip-existing` and `upload-zip --skip-existing` stop with an
  error when the folder has the file with another size, instead of
  uploading it as `name (1)`.
- `upload-zip` of a folder with a file of 4 GiB or more failed after
  zipping it all ("Large file option has not been set").
- Running `upload-dir --skip-existing` again right after a run (as the
  "session expired" message suggests) uploaded again, as `name (1)`, the
  files of the last minute, which listings didn't include yet. What
  `upload`, `upload-zip` and `upload-dir` upload is now noted in the
  cache, and `upload-dir` waits (up to 2 minutes) for it to be listed
  before deciding.
- A file over the part size whose name is 252–255 bytes long is refused
  before uploading: its parts' names (`name.001`) would be over O2
  Cloud's limit.
- The last line of `upload-dir` says "⚠" when files failed, names the
  local folder as "from", and counts updated files.
- `download` wrote straight to the final name, so a cut download left a
  file that looked complete, which the next run refused to overwrite. It
  still writes in place (an app may follow the file's size as progress),
  with a hidden `.<name>.o2dl` marker while incomplete: the same command
  now resumes it (by the part it stopped in, for a file in parts), and
  the finished file gets its original date.
- `download-dir --dry-run` now looks at the disk: it says what it would
  download, resume, skip or leave alone, and `--json` entries have
  `action` (`download`, `resume`, `skipped`, `failed`).
- `download-dir` joined any `x.001`, `x.002`… set, including someone's
  own volumes uploaded as files. It now joins only the parts this CLI
  makes (recognised by their size).
- `rm`, `mv`, `rename`, `cp` and `share` didn't know files in parts:
  the file's own name gave "not found", and a part's name acted on that
  part alone, breaking the file. They now take the file by its name and
  act on all its parts (put back as they were if one fails), and refuse
  a single part by name (its ID still works). `cp --json` of a file in
  parts has `parts` and `source_parts`.
- Requests that only read (`action=get`: listings, the change log) are
  tried again on any server error, and the folder list on `FOL-1000`,
  for about 30 s: a short O2 Cloud outage (500s for about a minute and a
  half) made any command fail.

## [0.15.2] — 2026-11-26

### Fixed
From a review and more tests of 0.15.1's `upload-dir --skip-existing`:
- A file in parts whose parts in the cloud don't match (it changed size
  but still has the same number of parts) failed to upload, or was left
  alone only if older locally. It is now left alone and reported like
  any file of another size (`differs`), older or not; `--dry-run` no
  longer says it would upload it.
- A file is `differs` only when uploading it would take a name already
  in use: a small file with only `name.001`… in the cloud (or a file
  that crossed the part size) is uploaded as before 0.15.1.
- `--json`: a `differs` entry has `parts` only when the cloud copy is in
  parts; duplicates of the name give the first one's `id`.
- `--skip-existing --dry-run` (without `--update`) counts a file of the
  same size that is older locally as `unchanged`, which is what the real
  run does, instead of `older`.

## [0.15.1] — 2026-11-26

### Changed
- `upload-dir --skip-existing` (without `--update`) no longer uploads a
  file the cloud has under the same name with another size: the server
  would keep both, the new one as `name (1)`. It is left alone and
  reported (`≠ … has another size than in the cloud`), with a count at
  the end; `--update` replaces it. `--json` entries for it have
  `"action": "differs"`, `"skipped": true` and the cloud copy's `id` (or
  `parts`); `--dry-run` counts them as "of another size".

## [0.15.0] — 2026-11-26

### Added
- `upload-dir --update`: also uploads files that changed locally (another
  size, or a modification date more than 2 s newer than the cloud copy's
  `modificationdate`) and replaces the cloud's copy (#24). The server
  renames an upload whose name is taken (`a (1).txt`, `a.dat (1).001`),
  so the new version is uploaded first, and only once that worked the
  old one goes to the trash and the new one gets its name: a failed
  upload leaves the old version alone. Files older locally are left
  alone. Implies `--skip-existing`. Files in parts are compared by their
  parts. Nothing is deleted that wasn't replaced.
- `upload-dir --dry-run`: says what it would upload, update or resume,
  and what is unchanged or older locally, without creating, uploading or
  moving anything; with `--json`, one entry per file with `dry_run` and
  the `action` it would take.
- `upload-dir --json` entries have an `action` field.

### Fixed
From a review of the new commands:
- `download-dir` of files uploaded in parts would fail on Windows: the
  partial file was opened in a mode Windows can't truncate. Parts now
  resume from the one a cut stopped in (Range within it), each retried
  once with a fresh URL, instead of starting over with URLs that could
  expire during a long file. The partial file is now `.<id>.o2part`, so a
  leftover from another version of the file is never resumed into this
  one (and long names still fit), and one larger than the file is
  started over. A session that expires while asking for URLs says so.
- `upload-dir --update` never replaces a newer cloud copy with an older
  local one of another size; replaces the parts of a file that is now
  under the part size (and the whole file of one now over it), instead
  of leaving the old version next to the new; and if a replacement fails
  halfway it takes back what it had uploaded, so it doesn't leave
  `name (1)` copies that pile up on each run. An expired session there
  stops the run as elsewhere.
- `upload-dir --dry-run` without `--skip-existing` reports every file as
  to upload, as the real run does, and lists files beyond O2 Cloud's
  limits; `older` entries carry the cloud copy's `id`.

### Changed
- Less memory on large folders: `upload-dir` and `download-dir` keep
  their per-file results only with `--json` (about 200 MB fewer for
  288,000 files), and listings no longer keep or ask for fields they
  don't use (the first full listing of such an account takes about a
  third of the memory).
- `upload-dir` asks for the folder list once instead of twice.
- `download-dir` joins files in parts looking only in their folder,
  not the whole account.
- `fix-jpeg` and the `MED-1000` explanation read only a JPEG's header
  (the first 256 KB), not the whole file.

## [0.14.1] — 2026-11-26

### Fixed
- `download-dir /` (the whole account) failed for every file in the root,
  such as the phone app's photos, when the `-o` folder didn't exist yet
  and nothing else created it: with `--ext`, or with no subfolders to
  download. The output folder is now always created first.

## [0.14.0] — 2026-11-26

### Added
- `download-dir <folder> [-o dir]`: downloads a folder and everything in
  it, or with `/` the whole account (which needs `-o`), the reverse of
  `upload-dir` (#24). Subfolders are recreated, empty ones too, files
  uploaded in parts are joined, and each file gets the modification date
  it had when uploaded. Files are written to `name.o2part` and renamed
  when complete; with `--skip-existing`, files already there with the
  same size are skipped without asking the server anything, and a cut
  download resumes where it stopped (O2 answers Range requests). Nothing
  local is overwritten. Names the local system can't use are adjusted,
  and names that clash ignoring case get the file's ID. `-j` downloads
  several at once, `--ext` keeps some extensions, `--dry-run` only counts,
  and `--json` lists every file. Download URLs are asked for 200 files at
  a time, just before downloading them; a session that expires stops the
  run cleanly. Tested with a round trip (nested, empty and accented
  folders, a file in parts): identical contents and dates.

### Changed
- Listings and the file cache now carry each file's `modificationdate`
  (the date sent at upload, which O2 keeps as is, while it replaces
  `creationdate` with a photo's EXIF date). The cache format changed, so
  the first command after updating lists the whole account once.

## [0.13.3] — 2026-11-26

### Added
- `fix-jpeg <files or folders> [--apply]`: finds JPEGs whose colour
  profile is split into chunks all labelled "1 of 1", which O2 Cloud
  rejects with `MED-1000`, and with `--apply` relabels the chunks in
  place (about 12 bytes; the image, the file's date and permissions are
  kept; written to a temporary file, checked, then renamed over it). A
  profile whose chunks don't add up is reported, not touched. Works on
  local files, without logging in. An upload rejected this way now says
  why and suggests the command. Found with photos from #24: 16 of 16
  repaired ones uploaded, all had failed before.
- O2 Cloud's limits, measured live, are checked before sending: a file
  name can be up to 255 bytes in UTF-8, a folder name up to 255
  characters, and folders can go 20 levels below the root. `upload-dir`
  leaves out the folders (with what's in them) and files that exceed
  them, says why, lists them in `--json` with an `error`, and uploads the
  rest instead of stopping at the first folder it can't create (#24).
  `mkdir`, `rename` and `upload` say which limit was hit, before any
  request where possible, and the server's `COM-1010`, `FOL-1015` errors
  get an explanation.
- Tests run on Windows in CI, with long path support turned off as on
  most PCs, including one with a local path of over 400 characters:
  paths over 260 characters work without `LongPathsEnabled` (#24).

### Fixed
- The Linux binary in the releases is built on Ubuntu 24.04 instead of
  the newest runner, so it runs on Ubuntu 24.04 LTS and other systems
  with glibc 2.39 or later. Recent ones needed glibc 2.43 and failed to
  start there (`GLIBC_2.43 not found`). The release now checks it.

### Maintenance
- Dependabot opens a weekly PR for the Rust dependencies and one for the
  GitHub Actions, and CI also runs weekly, so a new security advisory
  (`cargo audit`) shows up before a release instead of stopping it.
- Dependencies updated, among them `wry` 0.57 (the login window) and
  `dirs` 7 (same session and cache locations as before).

## [0.13.2] — 2026-11-25

### Fixed
- A session from the mobile-app login (the one that renews without SMS)
  no longer turns into a web session when a silent renewal fails: the
  login window that opens then uses the mobile-app login again, so the
  session keeps renewing itself. The OAuth client id and secret are now
  stored with the session in `auth.json` (readable only by the user,
  deleted by `logout`), so this works without the environment variables;
  `o2cloud login` also keeps using that login until `logout`. Without
  them, it now says it is falling back to the web login.
- The login says which kind it is, before the window opens and in the
  window's title: `app` (the session renews itself without SMS) or `web`
  (it ends after a while without use). `status` shows it too, and
  `status --json` has a new `login` field (`app` or `web`). The kind is
  still chosen as before, by `--oauth-client-id`/`--oauth-client-secret`
  (or their environment variables) or the stored session, never by
  asking. Logging in again after an app session no longer suggests the
  browser-cookie login, which would give a web session.

### Changed
- `upload-dir` skips empty `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp` and
  `.heic` files instead of sending them: O2 Cloud rejects every one
  (`MED-1050`, or `MED-1000` for HEIC), so they used to show up as
  failures. They are listed as `∅ … is empty: skipped`, counted in the
  summary, and in `--json` have `"skipped": true, "empty": true` and no
  `id`. `upload` of one fails at once with a clear message. Other empty
  files, which O2 Cloud accepts (`.txt`, `.pdf`, `.mp4`, `.heif`, `.svg`,
  RAW…), are still uploaded (#24).
- The README, `status` and error messages no longer say a session lasts
  about 24 hours: a web login ends after a while without use (it was
  gone after two hours idle in our tests), while one in constant use
  lasts much longer. The README also explains what O2 Cloud rejects
  (`MED-1050`/`MED-1000`).

## [0.13.1] — 2026-11-25

### Changed
- If the session expires in the middle of a command (web logins last
  about 24 hours), the command stops with "the session has expired"
  instead of a generic API error. `upload-dir` stops starting new files,
  lists the ones it didn't upload, and exits with an error; running it
  again with `--skip-existing` logs in and carries on (#24).
- Upload rejections `MED-1050` and `MED-1000` say what they mean: the
  server checks the file's content, so an empty or damaged file, or one
  whose format doesn't match its extension (a HEIC photo named `.jpg`),
  is rejected every time (#24).

## [0.13.0] — 2026-11-24

### Added
- `upload-dir --ext jpg,mp4` uploads only the files with those
  extensions (comma-separated or repeated, any case, with or without
  the dot; `tar.gz` and `*.jpg` work too), and creates only the
  subfolders that hold some. If no file matches it says so and creates
  nothing. The "Uploading" line says which extensions it is limited to.
  Contributed by @pinta101 in #24.

### Fixed
- `upload` and `upload-dir` no longer crash (`SystemTimeError`) on a file
  whose modification date is before 1970, which some files carry as an
  invalid date; it is sent as 1970-01-01 (#24, reported and fixed by
  @pinta101). A date after the year 9999 is sent as the end of 9999.

### Security
- Updated `cookie` to 0.18.3 (RUSTSEC-2026-0594) and `url` to 2.5.9
  (RUSTSEC-2026-0592).

## [0.12.1] — 2026-11-20

### Changed
- A numeric ID that isn't listed yet (a file uploaded a moment ago) is
  asked for less often while waiting for it: after 1 s, then 1.5 times
  longer each time up to 8 s, instead of every 2 s (#18). A file that is
  never found takes 16 requests instead of about 33 to give up.

### Fixed
- A file ID deleted since the latest upload (for example a second `rm`
  of the same ID) is reported as not found at once, from the server's
  change log, instead of after waiting 60 s for it to appear.

## [0.12.0] — 2026-11-19

### Added
- A cache of the account's file list (#20). Commands that need every
  file (`ls`, `find`, a bare file name, `rm` of a folder,
  `upload-dir --skip-existing`) list the whole account once, then only
  ask the server's change log (`/sapi/profile/changes`) what was added,
  changed, moved, renamed or deleted since, and fetch those files by ID:
  two requests for a `ls` instead of one per 1,000 files. The whole
  account is listed again once a week, or whenever the cache can't be
  brought up to date (unreadable, another account, an unknown kind of
  change, or a file the change log reported that the server never
  returns). It is readable only by the user, `logout` deletes it, and
  `O2CLOUD_CACHE=0` turns it off. Download URLs aren't cached.

### Changed
- Every file is listed in the same order (by name, ignoring case),
  whether it comes from the cache or the server.

## [0.11.0] — 2026-11-18

### Added
- `ls --folder-only` lists only the files directly in a folder, without
  listing the whole account: about a second instead of a minute or more
  on a large one (#20). Subfolders then show no file count or size
  (`null` in JSON), and the root leaves out the photos and videos the
  phone app uploaded, which have no folder.
- `ls --no-folder` lists only those photos and videos, which `ls` shows
  in the root (it still lists the whole account: the API can't filter
  them).

### Changed
- `ls`, `ls -t`, `ls -a` and `find` take time in proportion to the files
  and folders in the account, not their product: a root `ls` on an
  account with 46,000 files and a few hundred folders spent a minute of
  CPU adding up each subfolder's files (#20). The output is unchanged.
- The full file listing asks for four pages of 1,000 at a time after
  the first, instead of one after another.

## [0.10.0] — 2026-11-18

### Added
- `O2CLOUD_TRACE=1` logs each request a command sends to stderr
  (method, host, path, SAPI action, status and time; never the query
  string, headers or body, which hold the session keys) and how many
  there were.

### Changed
- Commands list far less of a large account (#18). Listing every file
  takes one request per 1,000 files, about 14 s on an account with
  46,000 of them, and every command given a file or folder did it:
  - A file ID is asked for on its own, and a file path (`/docs/a.txt`)
    lists only its folder (`download`, `rm`, `mv`, `rename`, `cp`,
    `share`).
  - A folder ID or path lists no files (`mv`, `rename`, `share`,
    `unshare`).
  - `mv`, `rename` and `cp` of a file check for a file with the same
    name by listing only the folder it goes to (O2 Cloud itself accepts
    duplicate names on a move or rename).
  - `upload --skip-existing`, and an upload in parts, list only the
    target folder.

  A bare file name, `ls`, `upload-dir --skip-existing`, `rm` of a folder
  and renaming a photo or video the phone app uploaded (those have no
  folder) still list the whole account.
- The session is checked with the folder list the command needs anyway
  instead of a request of its own, one request less per command.

## [0.9.0] — 2026-11-14

### Added
- Files over 4 GB, the most O2 Cloud takes in one upload, go up in
  parts of just under 4 GB named `name.001`, `name.002`, … (`upload`,
  `upload-dir` and `upload-zip`); the last part is always the smallest.
  `download name` joins them again, and refuses to if any part is
  missing (an upload cut short, or one not listed yet) or doesn't
  belong with the others; `cat name.0* > name` or 7-Zip join them too.
  Uploading a split file where parts of one with that name already are
  is refused, unless `--skip-existing` resumes the same file. A split
  upload that fails says which parts made it.
- `--skip-existing` for `upload`, `upload-dir` and `upload-zip`: files
  (and parts) already in the target folder with the same name and size
  are skipped, so an interrupted upload can be resumed without making
  `name (1)` copies. Only name and size are compared, not contents.
- `upload-dir -j <n>` uploads several files at a time (default 2, as
  the website does; up to 8).

### Changed
- `upload-dir` also skips the files the OS keeps for itself:
  `Thumbs.db`, `ehthumbs.db`, `desktop.ini`, `$RECYCLE.BIN`,
  `System Volume Information` and macOS's `Icon\r`.
- `--json` of the upload commands adds `skipped`, and `parts` instead
  of `id` for a file uploaded in parts; `download --json` gives
  `parts` for a joined file.

### Fixed
- `rm`, `mv`, `rename`, `cp`, `share` and `download` accept the ID of a
  file uploaded a moment ago. The server lists a new upload only after
  10–40 s, so they used to say "Not found"; now an ID newer than every
  listed file is asked for directly, waiting up to a minute for it.
- `mv` and `rename` work on photos, music and videos: the server wants
  a different request for each kind of file and refused the generic
  one (`MED-1005`). Other errors are shown as the server sent them.
- `rename` refuses to change a file's extension instead of saying it
  renamed the file: the server accepts the request but keeps the old
  extension (renaming `notes.pdf` to `notes.txt` leaves `notes.pdf`,
  and `a.JPG` to `a.jpg` leaves `a.JPG`).
- `rm`, `mv`, `rename` and `cp` wait and retry while the server is
  still processing a file uploaded a moment ago (`MED-1017`) instead of
  failing.
- `download` of a file uploaded a moment ago no longer fails with
  "has no download URL": the URL is asked for by ID.
- `download` checks it received the whole file, and no longer saves
  the server's JSON error as the downloaded file; either way the
  partial file is removed.
- `rm` of a folder reports the server's refusal instead of success.
- Calls the server answers with 429 (too many requests) or 503 are
  retried up to three times with a growing wait, uploads included, so
  large `upload-dir` runs and scripts don't stop on a busy server.
  Requests that create something (a folder, a copy, a link) aren't
  retried on 503, which can come after the server acted.

## [0.8.0] — 2026-10-30

### Added
- `--json` for `find`, `upload`, `upload-dir`, `upload-zip`, `download`,
  `mkdir`, `cp`, `share`, `shares`, `status` and `quota`: one JSON value
  on stdout (search results, new IDs, links, where a download was saved,
  storage figures, the session state), so scripts and other apps don't
  have to parse the messages. Progress stays on stderr. (#9)

### Fixed
- `ls`, `find`, `download`, `rm`, `mv`, `rename`, `cp`, `share` and
  `unshare` no longer fail on accounts with a file that has a negative
  creation date (some old phone videos). That date is treated as unknown,
  so the upload date is shown instead. (#7)

## [0.7.0] — 2026-10-24

### Added
- `mkdir <folder>` creates a folder; `-p` also creates missing parent
  folders and doesn't fail if the folder already exists.
- `mv` and `rename` work on folders too, with everything inside them.
  Moving a folder into itself or one of its subfolders, and moving or
  renaming the root folder, are refused.
- `cp <file> <folder>` copies a file on the server, without downloading
  it; `--name` gives the copy another name. Folders can't be copied yet.
- `quota` shows the storage used, free and in the trash, and `status`
  shows the storage used too.

### Changed
- `mkdir`, `mv`, `cp` and `rename` refuse to create a second file or
  folder with a name already used in the same place. The server would
  allow two folders with the same name, or rename the new one to
  `name (1)` without saying so.

## [0.6.0] — 2026-10-24

### Added
- `ls` sorts files by name (case-insensitive) instead of the server's
  order; `-s size` / `-s date` sort by size or date, and `-r` reverses.
- `ls` ends with a summary (`2 folders, 4039 files, 12.1 GB in total`),
  and each folder shows how many files it holds and their size,
  subfolders included.
- `ls -l` adds each file's date: when it was taken or created, or else
  when it was uploaded (UTC). `-t -l` shows it in the tree too.
- `ls -n <pattern>` shows only names matching a `*`/`?` pattern
  (case-insensitive), e.g. `-n '*.heic'`.
- `ls --limit <n>` shows at most `n` files; the summary still counts all
  of them and says how many were left out.
- `ls --json` prints the listing as JSON for scripts.

### Changed
- `ls -a <folder>` lists every file under that folder; before, `-a`
  ignored the folder and listed the whole account (still the default
  without one).
- The ID column widens to fit the IDs shown, so long file IDs no longer
  push the columns out of line.

### Fixed
- `rm -r` failed on folders with many files with "Number of items in the
  request is over the limit (MED-1025)": batches of 1000 IDs are too
  many for the server. It now halves the batch and retries until the
  server accepts it.
- `rm -r` stopped with "File already soft deleted (MED-1022)" when a
  file in a batch was already in the trash; it now skips those files and
  deletes the rest.
- Piping output into a command that exits early (`o2cloud ls | head`)
  no longer crashes with "failed printing to stdout: Broken pipe".

## [0.5.0] — 2026-10-23

### Added
- **Advanced: silent session renewal (no repeated SMS).** `login
  --oauth-client-id <id> --oauth-client-secret <secret>` — or the
  `O2CLOUD_OAUTH_CLIENT_ID` / `O2CLOUD_OAUTH_CLIENT_SECRET` environment
  variables — logs in through the mobile app's OAuth flow and stores a
  refresh token, so the session renews itself without a new SMS code for
  as long as the token is valid (about 60 days). You still enter the phone
  number and SMS code once in the login window. The default web login and
  the browser-cookie login are unchanged and still can't be renewed.

### Security
- The CLI does **not** ship the mobile app's OAuth client credentials — you
  supply them — and this mode makes the CLI identify to Telefónica as the
  official app, so it is advanced and use-at-your-own-risk: it may stop
  working if the credentials or the flow change. Pass the credentials via
  the environment variables rather than the flags so they don't land in
  your shell history, and never commit them.

## [0.4.0] — 2026-10-23

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
