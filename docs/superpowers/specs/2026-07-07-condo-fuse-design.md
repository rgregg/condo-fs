# condo-fuse — Design

**Date:** 2026-07-07
**Status:** Approved (design), pending implementation plan
**Author:** Ryan Gregg (with Claude)

## Summary

`condo-fuse` mounts a [Condo Control](https://app.condocontrol.com) association **File Library** as a
read-only POSIX filesystem via FUSE, so the library can be browsed and read with ordinary
file tools instead of the web UI. It is built as a **reusable Rust library** (a FUSE-agnostic
Condo Control API client) plus a thin `condo-fuse` mount binary.

Scope for this project is **read-only**: browse folders, read/copy/open files. No create,
rename, move, or delete. The web API supports writes, and the client is structured so write
support could be added later, but it is explicitly out of scope here.

## Goals

- Mount a Condo Control File Library subtree (rooted at a given folder ID) as a read-only filesystem.
- Take the user's login credentials from a file and handle session auth transparently.
- Present folders and files with clean, stable, human-readable names.
- Be a reusable component: the API client is independent of FUSE and independently testable.
- Sensible caching with easily configurable freshness/location knobs.

## Non-goals

- Any write operation (upload, rename, move, delete, tag, permissions).
- Search UI, thumbnails, or rendering — plain file access only.
- Cross-platform GUI. Target is Linux (FUSE). macOS (macFUSE) is a possible later nicety, not a goal.
- Mirroring web-UI features beyond folder/file browsing and download.

## Reverse-engineered API contract

Confirmed live on 2026-07-07 against `https://app.condocontrol.com`. ASP.NET MVC app,
session-cookie auth.

### Authentication
- `GET /login` first to obtain an `ASP.NET_SessionId` cookie.
- `POST /login/login-post` as **`multipart/form-data`** with fields:
  `Username`, `Password`, `SaveEmail=false`, `Lang=en`, `RedirectURL=`.
- Success → `302` to `/my/my-home` and sets auth cookies **`CCCookie`** and `_ccc_analytic`.
- Failure → `302` back to `/login` with no `CCCookie`.
- Session expiry is detectable: authenticated requests `302` to `/login?NextPage=…` when the
  session is gone.

### Credentials file
Plain `KEY=VALUE` lines. **Note the exact keys:**
```
USERNAME=<email>
PASSWORD=<password>   # may contain special chars such as ^ and #
```
The password key is `PASSWORD` (not `PASS`). Values may contain shell/format-hostile
characters, so the parser must read the file directly (not via shell `source`) and take
everything after the first `=` verbatim, trimming only a trailing newline.

### List a folder — primary endpoint
`GET /library/get-file-list` with query params (all required; missing params → `302` to
`/Error/invalid-address`):
```
mode=0
folderID=<id>
searchString=
fileTypeSelectID=0
startDate=
endDate=
newSearch=False
```
Header `X-Requested-With: XMLHttpRequest` is sent by the site (include it).
Returns a JSON **array** mixing folders and files:
```json
{
  "ID": 5369528,
  "Key": "9E825A05-B799-4A3A-8635-9C9B19A66ADB",
  "Thumbnail": "/shared/images/icons/pdf-128x128.png",
  "Name": "01/09/25 Board Minutes",
  "Desc": null,
  "Date": "2025-01-18 02:41:25",
  "Link": "https://app.condocontrol.com/library/view-file.aspx?FileRecordID=5369528&Key=...",
  "Options": 1
}
```
- **`Options`**: `2` = folder, `1` = file. (Corroborated by `Thumbnail` = `folder.gif` and
  `Link` containing `view-folder` for folders; `view-file.aspx` for files.)
- Folders: `Date` empty, `Link` = `…/library/view-folder?folderID=<ID>`.
- Files: real `Date` (`YYYY-MM-DD HH:MM:SS`), `Key` GUID present, `Thumbnail` encodes type.
- **`Name` has no extension and may contain `/`.**
- **No file size is provided here.**

### Full folder tree (optional optimization)
`GET /library/get-folder-hierarchy?folderID=<id>` → `{ "js": "success", "message": "<nested <ul>/<li> HTML>" }`
with `view-folder?folderID=N` links and folder names. Useful for pre-warming the folder tree;
not required for correctness (lazy `get-file-list` per folder is sufficient).

### Download a file
`GET /library/download-file?fileRecordID=<ID>`:
- `200`, raw bytes (e.g. `Content-Type: application/pdf`).
- `Content-Length` present → authoritative size.
- `Content-Disposition: attachment; filename="01/09/25 Board Minutes.pdf"` → real filename **with**
  extension.
- **No `Accept-Ranges`; Range requests are ignored** (a `Range: 0-0` returned the full length
  with a `200`). Partial reads are therefore not possible server-side; a read requires
  downloading the whole file.

## Architecture

Rust workspace: a library crate (`condo-fuse`) + binary (`bin/condo-fuse`). Modules:

### `client` — reusable Condo Control API client (FUSE-agnostic)
- Trait `CondoClient` with impl `HttpCondoClient` over `reqwest::blocking` (or async + a small
  runtime; blocking is simpler for FUSE's synchronous callbacks) with a cookie store.
- Methods:
  - `login(&Credentials) -> Result<()>`
  - `list_folder(folder_id) -> Result<Vec<Entry>>` where `Entry` is
    `Folder { id, name }` or `File { id, key, name, date, thumbnail }`.
  - `file_meta(file_id) -> Result<FileMeta { size, filename }>` — header-only request to learn
    size/real name without downloading the body (see "File size" below).
  - `download_file(file_id, sink) -> Result<u64>` — streams bytes to a writer, returns size.
- **Transparent re-auth**: if any request `302`s to `/login`, re-run `login()` once and retry;
  a second failure surfaces an error.
- Depending only on the trait keeps the FUSE layer testable with a mock and the client reusable
  by non-FUSE tools.

### `model` — inode/tree state
- `inode ↔ Condo id` bimap and a `Node` enum (`Folder { id }` / `File { id, key, date, size: Option<u64> }`).
- Owns **name sanitization + collision resolution** (below). Once a name is assigned to an inode
  it stays stable for the session.

### `cache`
- **Content cache** (on disk by default): keyed by `fileRecordID + date`. A changed `date`
  invalidates the entry, forcing re-download. `--cache-mode` selects `disk` (default),
  `memory` (no local copies), or `persistent` (longer TTL / manual refresh).
- **Metadata/listing cache** (in memory): per-folder listing with a TTL (`--meta-ttl`,
  default 60s) so new uploads appear without remount.

### `fs` — FUSE implementation
Implements `fuser::Filesystem` read-only ops: `lookup`, `getattr`, `readdir`, `open`, `read`,
`release`. Pure translation: FUSE call → `model`/`cache`/`client`. All entries are
`0o444` files / `0o555` dirs owned by the mounting uid.

### `config`
Resolves credentials (default path `~/tokens/condo-control.txt`), root folder id, mountpoint,
cache dir (default `~/.cache/condo-fuse`), cache mode, and TTLs from CLI flags + optional
config file + env.

### `bin/condo-fuse`
```
condo-fuse mount \
  --credentials ~/tokens/condo-control.txt \
  --root 137473 \
  /mnt/condo \
  [--cache-dir DIR] [--cache-mode disk|memory|persistent] [--meta-ttl SECONDS] [--foreground]
```

## Key behaviors

### Name handling
- File/folder `Name` may contain `/` and (for files) has no extension.
- Sanitize `/` → `-` (character configurable). Trim/normalize whitespace.
- Append an extension inferred from `Thumbnail` at listing time (e.g. `pdf-128x128.png` → `.pdf`)
  so names show extensions before any download. If the icon is unknown, leave no extension;
  a later `Content-Disposition` may reveal one but the session name stays stable.
- Resolve duplicate sanitized names within a folder by appending ` (2)`, ` (3)`, … in listing
  order.

### File size for `getattr`
Listings lack size and Range is unsupported. On the first `getattr`/`open` of a file, call
`file_meta()` — a request whose body is **not** consumed, reading only `Content-Length` (and
`Content-Disposition`). Cache the size on the inode. During implementation, validate that the
server honors an early-aborted GET (or a `HEAD`) without transferring the whole body; if it does
not, fall back to a full download-to-cache on first `getattr` and serve `read` from the cache.
`open` always ensures the full file is in the content cache; `read` serves byte ranges from that
local file.

### Freshness
Directory listings refresh after `--meta-ttl`. A file's cached content is re-downloaded when its
listing `date` changes.

## Error handling
- Auth failure at mount → clear, actionable error; exit non-zero.
- Session expiry mid-operation → transparent re-login + one retry; if still failing → `EIO`.
- Network/HTTP errors → `EIO`; logged to stderr.
- Stale entry (file/folder no longer present) → `ENOENT`, and trigger a listing refresh so the
  next lookup is correct.
- Unknown/edge JSON → skip the row with a logged warning rather than failing the whole listing.

## Testing
- **Unit tests on recorded fixtures** captured this session (real `get-file-list` JSON,
  `get-folder-hierarchy` HTML, `download-file` headers): credential-file parsing, folder-list
  parsing, folder/file discrimination via `Options`, hierarchy HTML parsing, name
  sanitization/collision, extension inference, cache-key/invalidation logic.
- **FUSE layer against a mock `CondoClient`** — no network; verify `lookup`/`readdir`/`getattr`/
  `read` semantics, including size resolution and stale-entry `ENOENT`.
- **One opt-in, env-gated live integration test** that logs in with real credentials and lists
  the root folder. Not run in CI.

## Open questions / to validate during implementation
1. Whether `HEAD` or an early-aborted GET yields `Content-Length` without a full transfer
   (drives the `file_meta` implementation; full-download fallback specified above).
2. `reqwest` blocking vs async under FUSE's synchronous callbacks (lean blocking for simplicity).
3. macFUSE support later — not in scope now.
