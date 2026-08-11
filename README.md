# condo-fs

Mount a [Condo Control](https://app.condocontrol.com) association **File Library** as a
read-only [FUSE](https://www.kernel.org/doc/html/latest/filesystems/fuse.html) filesystem,
so you can browse and read library documents with ordinary file tools instead of the web UI.

It is a Cargo workspace: a reusable, FUSE-agnostic Condo Control API client crate
(`condo-api`) plus a thin `condo-fs` mount binary built on top of it. See
[Workspace layout](#workspace-layout).

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
./target/release/condo-fs mount \
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
| `--cache-dir <dir>` | `~/.cache/condo-fs` | On-disk content cache |
| `--meta-ttl <seconds>` | `60` | How long directory listings are cached before refetching |

Set `RUST_LOG=info` (or `debug`) for logging.

## Install & auto-mount on login (systemd)

Mount the library automatically every time you log in, using a systemd **user** service.

1. Build and install the binary to a stable location:

   ```bash
   cargo build --release
   install -Dm755 target/release/condo-fs ~/.local/bin/condo-fs
   ```

2. Install the service unit (a template lives in [`packaging/condo-fs.service`](packaging/condo-fs.service)):

   ```bash
   mkdir -p ~/.config/systemd/user
   cp packaging/condo-fs.service ~/.config/systemd/user/
   ```

   Edit `~/.config/systemd/user/condo-fs.service` and set `--root` to your library's root
   folder ID (and adjust the credentials path or mountpoint if you use different ones).

3. Enable and start it:

   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now condo-fs.service
   ```

The library is now mounted at `~/condo` and will remount on every login.

> To also start it at boot **before** you log in (e.g. for SSH access), enable lingering:
> `sudo loginctl enable-linger "$USER"`.

Managing the service:

```bash
systemctl --user status condo-fs      # is it running?
systemctl --user restart condo-fs     # remount (after changing options or updating the binary)
systemctl --user stop condo-fs        # unmount now
systemctl --user disable condo-fs     # stop auto-mounting on login
journalctl --user -u condo-fs -f      # live logs
```

To upgrade after pulling new code:

```bash
cargo build --release
install -Dm755 target/release/condo-fs ~/.local/bin/condo-fs
systemctl --user restart condo-fs
```

## Windows access via WSL2

There is no native Windows build (Windows has no FUSE), but condo-fs runs **unmodified inside
WSL2**, and the mount is then reachable from Windows Explorer. WSL1 will not work — it has no
`/dev/fuse`.

1. **Install WSL2** with a Linux distro. In an elevated PowerShell:

   ```powershell
   wsl --install
   ```

   Reboot if prompted, then confirm the distro is version 2:

   ```powershell
   wsl -l -v
   ```

2. **Inside the WSL distro**, install FUSE, then build and install condo-fs exactly as in the
   sections above:

   ```bash
   sudo apt update && sudo apt install -y fuse3
   # ... then the cargo build / install steps from "Install & auto-mount on login"
   ```

3. **Put your credentials on the Linux side** (e.g. `~/tokens/condo-control.txt` *within* the
   distro, not under `/mnt/c/...`), so file permissions work correctly. `chmod 600` it.

4. **(Optional) Enable systemd** so the auto-mount user service works. Edit `/etc/wsl.conf`:

   ```ini
   [boot]
   systemd=true
   ```

   Then from PowerShell `wsl --shutdown`, reopen the distro, and set up the service as in
   [Install & auto-mount on login](#install--auto-mount-on-login-systemd). If you'd rather not
   enable systemd, just run the mount manually (the [Usage](#usage) command) or add it to your
   shell profile.

5. **Access from Windows.** Open File Explorer and browse to:

   ```
   \\wsl$\Ubuntu\home\<you>\condo
   ```

   (or `\\wsl.localhost\Ubuntu\home\<you>\condo` on newer Windows). Replace `Ubuntu` with your
   distro name. You can right-click → *Map network drive* to give it a drive letter.

**Notes:**
- The mount only exists while it is running inside WSL — via the systemd service or your manual
  command. If WSL shuts down when idle, reopening the distro restarts the service (with systemd
  enabled + lingering).
- Keep the on-disk cache on the Linux side (the default `~/.cache/condo-fs` already is);
  reading files stays read-only as usual.

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

## Workspace layout

This repo is a Cargo workspace with three crates:

- **`condo-api`** — the Condo Control HTTP client: login/session handling, re-auth on
  session expiry, and the response models (`Entry`, `FileMeta`) and errors (`ClientError`).
  FUSE-agnostic and reusable by any consumer.
- **`condo-fs`** — the FUSE filesystem and `condo-fs` mount binary, built on `condo-api`.
- **`condo-mcp`** — a read-only MCP server exposing the same Condo Control File Library
  over `condo-api`, for MCP clients such as Claude Code. See [condo-mcp](#condo-mcp) below.

## Using the client as a library

The `condo_api::CondoClient` trait (impl `HttpCondoClient`) is independent of FUSE
and can be reused directly:

```rust
use condo_api::{CondoClient, Credentials, HttpCondoClient};

let creds = Credentials::from_file("~/tokens/condo-control.txt".as_ref())?;
let client = HttpCondoClient::new("https://app.condocontrol.com", creds)?;
client.login()?;
let entries = client.list_folder(137473)?;
```

## condo-mcp

`condo-mcp` is a read-only [MCP](https://modelcontextprotocol.io) server that exposes a
Condo Control association to MCP clients (Claude Code, Claude Desktop, etc.) as query
tools. It speaks MCP over stdio and shares one authenticated `condo-api` session across
every tool call. There is no write path: every tool is a query, and nothing it does can
post, submit, or change anything in Condo Control.

Currently available tools:

- **`list_folder`** — list the folders and files directly inside one File Library folder.
  Takes an optional `folder_id`; omit it to list the configured root folder. Returns each
  entry's kind (`folder`/`file`), id, name, and modification date (files only). File
  contents cannot be read yet.

Build it the same way as `condo-fs`:

```bash
cargo build --release --bin condo-mcp
```

Register it with an MCP client by pointing at the built binary. For Claude Desktop or any
client that reads an `mcpServers` config block:

```json
{
  "mcpServers": {
    "condo": {
      "command": "/path/to/condo-fs/target/release/condo-mcp",
      "args": ["--credentials", "/home/you/tokens/condo-control.txt"]
    }
  }
}
```

Or, for Claude Code:

```bash
claude mcp add condo -- /path/to/condo-fs/target/release/condo-mcp
```

### condo-mcp options

| Flag | Default | Meaning |
|------|---------|---------|
| `--credentials <path>` | `~/tokens/condo-control.txt` | Credentials file (same format as `condo-fs`; see [Credentials](#credentials)) |
| `--base-url <url>` | `https://app.condocontrol.com` | Condo Control base URL |
| `--root <id>` | `137473` | Root library folder ID (same meaning as `condo-fs`'s `--root`) |

Login is lazy — the credentials file is only read and used on the first tool call, so a
missing or bad credentials file surfaces as a normal tool error rather than a startup
crash. All logging goes to stderr; stdout carries only the MCP protocol.

## condo-probe

`condo-probe` is a GET-only exploration tool for discovering Condo Control endpoints that
`condo-api` doesn't map yet. You give it an explicit list of paths (nothing is crawled and
no path is ever derived from a response body); it logs in with your real credentials,
fetches each one, and dumps the raw response verbatim to `--out-dir` (default `probe-out`)
for inspection.

It is feature-gated because it is a discovery/debugging tool, not something built by
default:

```bash
cargo run -p condo-api --features probe --bin condo-probe -- \
    --xhr /announcements/get-announcement-list /calendar/get-event-list
```

By default it waits 1000ms between requests (`--delay-ms`); this hits a production system
you don't own, so raise the delay rather than lower it, and never point it at a tight loop
of paths without a delay.

**Warnings:**

- Output written to `--out-dir` contains **real resident data** pulled from a live
  association. It must never be committed — `probe-out/` is gitignored at every depth, but
  double-check `git status` before committing anything from a probing session regardless.
- Dumps include response headers verbatim, which means **`set-cookie` headers — live
  session tokens — are written to disk** in the output files, with your process's default
  file permissions. Treat the output directory like you would the credentials file itself.

## Development

```bash
cargo test --all --all-features     # unit tests (no network), across the whole workspace, including condo-probe
CONDO_LIVE=1 cargo test -p condo-api --test live_smoke -- --nocapture   # opt-in live login/list test
```

Design and implementation notes live in `docs/superpowers/`.
