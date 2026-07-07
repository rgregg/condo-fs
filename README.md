# condo-fuse

Mount a [Condo Control](https://app.condocontrol.com) association **File Library** as a
read-only [FUSE](https://www.kernel.org/doc/html/latest/filesystems/fuse.html) filesystem,
so you can browse and read library documents with ordinary file tools instead of the web UI.

It is a reusable Rust library (a FUSE-agnostic Condo Control API client) plus a thin
`condo-fuse` mount binary.

## Status

Read-only. You can list folders and read/copy files. There is no create, rename, move, or
delete — by design.

## Requirements

- Linux with FUSE (`fusermount3` on `PATH`, `/dev/fuse` present). On Debian/Ubuntu:
  `sudo apt install fuse3`. No `libfuse-dev` is needed — the crate mounts through
  `fusermount3`.
- Rust ≥ 1.95 to build.

## Credentials

Create a file (default `~/tokens/condo-control.txt`) with your Condo Control login:

```
USERNAME=you@example.com
PASSWORD=your-password
```

Keys are `USERNAME` and `PASSWORD`. The password is read verbatim (special characters are
fine). Keep this file readable only by you (`chmod 600`).

## Build

```bash
cargo build --release
```

## Usage

```bash
mkdir -p /tmp/condo
./target/release/condo-fuse mount \
  --credentials ~/tokens/condo-control.txt \
  --root 137473 \
  /tmp/condo

# in another terminal:
ls /tmp/condo
cp "/tmp/condo/Board of Directors/…/Minutes.pdf" ~/Downloads/

# unmount when done:
fusermount3 -u /tmp/condo
```

Find your library's root folder ID in the web UI URL:
`…/library/view-folder?folderID=<THIS NUMBER>`.

### Options

| Flag | Default | Meaning |
|------|---------|---------|
| `--credentials <path>` | `~/tokens/condo-control.txt` | Credentials file |
| `--root <id>` | `137473` | Root library folder ID |
| `--cache-dir <dir>` | `~/.cache/condo-fuse` | On-disk content cache |
| `--meta-ttl <seconds>` | `60` | How long directory listings are cached before refetching |

Set `RUST_LOG=info` (or `debug`) for logging.

## How it works

- Logs in via Condo Control's form endpoint and holds the session cookie; re-authenticates
  transparently if the session expires.
- Lists folders through the `get-file-list` endpoint. Folder vs file is determined by the
  entry's link URL (`view-folder` vs `view-file`), not by the `Options` field (which is a
  permissions bitmask).
- File names are sanitized (`/` → `-`) and given an extension inferred from the file's icon;
  duplicate names within a folder get a ` (2)` suffix.
- Files download whole on first read (the server does not support HTTP range requests) and are
  cached on disk keyed by file id + modified date, so an unchanged file is fetched only once.

## Using the client as a library

The `condo_fuse::client::CondoClient` trait (impl `HttpCondoClient`) is independent of FUSE
and can be reused directly:

```rust
use condo_fuse::client::{CondoClient, HttpCondoClient};
use condo_fuse::credentials::Credentials;

let creds = Credentials::from_file("~/tokens/condo-control.txt".as_ref())?;
let client = HttpCondoClient::new("https://app.condocontrol.com", creds)?;
client.login()?;
let entries = client.list_folder(137473)?;
```

## Development

```bash
cargo test          # unit tests (no network)
CONDO_LIVE=1 cargo test --test live_smoke -- --nocapture   # opt-in live login/list test
```

Design and implementation notes live in `docs/superpowers/`.
