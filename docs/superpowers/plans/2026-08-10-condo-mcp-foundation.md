# condo-mcp Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split this crate into a workspace with a shared `condo-api`, add a GET-only live-probe tool for endpoint discovery, and stand up a working read-only `condo-mcp` stdio server carrying its first tool.

**Architecture:** `condo-api` holds authentication, the HTTP client, and response models, using blocking `reqwest`. `condo-fs` (FUSE) consumes it unchanged. `condo-mcp` is a Tokio binary using the `rmcp` SDK over stdio; each tool wraps a blocking client call in `spawn_blocking`, sharing one `Arc<HttpCondoClient>` and one authenticated session for the process lifetime.

**Tech Stack:** Rust 2021 (1.95), blocking `reqwest` 0.12 + rustls, `rmcp` 3, `tokio` 1, `schemars` 1, `clap` 4, `httpmock` 0.7 for tests.

**Source spec:** `docs/superpowers/specs/2026-08-10-condo-mcp-design.md`

## Scope of this plan

This plan covers **Phase 1a**: the refactor, the probe, and the server skeleton. It stops at a
discovery checkpoint.

The announcements and calendar tools are **deliberately not planned here**. Their endpoints,
JSON shapes, and pagination are unknown — they are what Task 2's probe exists to discover.
Writing parser code against imagined JSON would be fiction. Once probing produces committed
fixtures, a second plan (`2026-08-10-condo-mcp-domains.md`) covers those two domains against
real data.

Everything in this plan is fully determined and blocks on nothing.

## Global Constraints

- **Read-only.** No code in any crate may issue an HTTP method other than GET or the single
  existing POST to `/login/login-post`. No new POST, PUT, PATCH, or DELETE. This is the
  central safety property of the project.
- Rust edition 2021, `rust-version = "1.95"`, license MIT — all three crates.
- `condo-api` stays **blocking**. Do not convert it to async; `condo-fs` depends on it being
  blocking.
- The password is never logged, never placed in an error message, and never serialized. The
  hand-written redacting `Debug` impl on `Credentials` must survive every move.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  and `cargo test --all` must pass. CI already runs exactly these and needs no changes.
- `condo-fs` behavior does not change in this plan. Its existing tests must pass unmodified
  except for import paths.
- In `condo-mcp`, **stdout is the MCP wire protocol**. All logging goes to stderr. A single
  `println!` breaks the server.

---

## File Structure

**Created:**

| File | Responsibility |
|---|---|
| `Cargo.toml` (root) | Workspace manifest: members, shared package metadata, shared dep versions |
| `condo-api/Cargo.toml` | Shared crate manifest; `probe` feature gating the probe binary |
| `condo-api/src/lib.rs` | Module declarations and flat re-exports |
| `condo-api/src/error.rs` | `ClientError` |
| `condo-api/src/paths.rs` | `expand_tilde` |
| `condo-api/src/credentials.rs` | `Credentials`, `CredentialsError` (moved) |
| `condo-api/src/client/mod.rs` | `CondoClient` trait, `HttpCondoClient`, `login`, `with_reauth`, `get_raw` |
| `condo-api/src/client/library.rs` | File Library request/response methods |
| `condo-api/src/model/mod.rs` | Model re-exports |
| `condo-api/src/model/library.rs` | `Entry`, `FileMeta`, `parse_file_list` (moved) |
| `condo-api/src/bin/condo-probe.rs` | GET-only live exploration tool |
| `condo-mcp/Cargo.toml` | MCP server manifest |
| `condo-mcp/src/main.rs` | CLI parsing, logging to stderr, stdio transport |
| `condo-mcp/src/lib.rs` | `Args`, module declarations |
| `condo-mcp/src/server.rs` | `CondoMcp`, shared client, lazy login, error mapping |
| `condo-mcp/src/tools/library.rs` | `list_folder` tool and its wire types |
| `condo-mcp/tests/server.rs` | In-process MCP client tests |

**Moved:** `src/{credentials,client,model}.rs` → `condo-api/`; `src/{fs,vfs,cache,names,config}.rs`,
`src/bin/condo-fs.rs` → `condo-fs/src/`; `tests/fixtures/*` and `tests/live_smoke.rs` → `condo-api/tests/`.

**Modified:** `condo-fs/src/{vfs,fs,lib,config}.rs` (imports only), `README.md`, `.gitignore`.

---

### Task 1: Extract `condo-api` and convert the repo to a workspace

This task is a pure refactor. Its test is the **existing** test suite passing unchanged —
there is no new behavior to test-drive. Every assertion that exists today must still hold
afterward, which is exactly the property a refactor needs.

**Files:**
- Create: `Cargo.toml` (root, replacing the current one), `condo-api/Cargo.toml`,
  `condo-api/src/lib.rs`, `condo-api/src/error.rs`, `condo-api/src/paths.rs`,
  `condo-api/src/client/mod.rs`, `condo-api/src/client/library.rs`,
  `condo-api/src/model/mod.rs`, `condo-api/src/model/library.rs`, `condo-fs/Cargo.toml`
- Move: `src/credentials.rs` → `condo-api/src/credentials.rs`; `tests/fixtures/` →
  `condo-api/tests/fixtures/`; `tests/live_smoke.rs` → `condo-api/tests/live_smoke.rs`;
  `src/{cache,fs,names,vfs,config,lib}.rs` → `condo-fs/src/`; `src/bin/condo-fs.rs` →
  `condo-fs/src/bin/condo-fs.rs`
- Delete after moving: `src/client.rs`, `src/model.rs`
- Modify: `condo-fs/src/lib.rs`, `condo-fs/src/vfs.rs:1-9,337`, `condo-fs/src/fs.rs:1`,
  `condo-fs/src/config.rs`, `condo-fs/src/bin/condo-fs.rs:1-8`, `README.md`

**Interfaces:**
- Consumes: nothing (first task).
- Produces, all reachable from the `condo_api` crate root:
  - `condo_api::Credentials { username: String, password: String }`, `Credentials::from_file(&Path) -> Result<Credentials, CredentialsError>`
  - `condo_api::ClientError` with variants `Http(reqwest::Error)`, `Auth`, `Parse(serde_json::Error)`, `Io(std::io::Error)`, `NotFound`
  - `condo_api::Entry` (enum: `Folder { id: u64, name: String }`, `File { id: u64, key: String, name: String, date: String, thumbnail: String }`), `condo_api::FileMeta { size: u64, filename: Option<String> }`
  - `condo_api::CondoClient` trait: `login(&self) -> Result<(), ClientError>`, `list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError>`, `file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError>`, `download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError>`
  - `condo_api::HttpCondoClient::new(base_url: impl Into<String>, creds: Credentials) -> Result<HttpCondoClient, ClientError>`
  - `condo_api::paths::expand_tilde(p: &str) -> PathBuf`

- [ ] **Step 1: Create the directories and move files with git**

```bash
mkdir -p condo-api/src/{client,model} condo-api/tests condo-fs/src/bin
git mv src/credentials.rs condo-api/src/credentials.rs
git mv src/client.rs condo-api/src/client/mod.rs
git mv src/model.rs condo-api/src/model/library.rs
git mv tests/fixtures condo-api/tests/fixtures
git mv tests/live_smoke.rs condo-api/tests/live_smoke.rs
git mv src/cache.rs src/fs.rs src/names.rs src/vfs.rs src/config.rs src/lib.rs condo-fs/src/
git mv src/bin/condo-fs.rs condo-fs/src/bin/condo-fs.rs
rmdir src/bin src 2>/dev/null || true
```

Use `git mv` rather than `mv` so history follows the files.

- [ ] **Step 2: Write the root workspace manifest**

Replace `Cargo.toml` entirely. `condo-mcp` is **not** a member yet — listing a member whose
directory does not exist is a hard cargo error. Task 3 adds it.

```toml
[workspace]
resolver = "2"
members = ["condo-api", "condo-fs"]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.95"
license = "MIT"
repository = "https://github.com/rgregg/condo-fs"

[workspace.dependencies]
reqwest = { version = "0.12", default-features = false, features = ["blocking", "cookies", "multipart", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
log = "0.4"
env_logger = "0.11"
clap = { version = "4", features = ["derive"] }
dirs = "5"
httpmock = "0.7"
tempfile = "3"
```

- [ ] **Step 3: Write `condo-api/Cargo.toml`**

```toml
[package]
name = "condo-api"
description = "Client for the Condo Control association web application"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
log = { workspace = true }
dirs = { workspace = true }

[dev-dependencies]
httpmock = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 4: Write `condo-api/src/error.rs`**

Cut the `ClientError` enum out of `condo-api/src/client/mod.rs` and paste it here verbatim,
adding the imports it needs:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("authentication failed")]
    Auth,
    #[error("parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found")]
    NotFound,
}
```

- [ ] **Step 5: Write `condo-api/src/paths.rs`**

Cut `expand_tilde` out of `condo-fs/src/config.rs` and paste it here:

```rust
use std::path::PathBuf;

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}
```

- [ ] **Step 6: Write `condo-api/src/model/mod.rs` and fix `model/library.rs`**

`condo-api/src/model/mod.rs`:

```rust
pub mod library;

pub use library::{parse_file_list, Entry, FileMeta};
```

`condo-api/src/model/library.rs` needs no edits — it has no `crate::` imports and its tests
read `tests/fixtures/*.json`, which resolve against the package root (`condo-api/`) where
the fixtures now live.

- [ ] **Step 7: Split the client — move the File Library methods into `client/library.rs`**

A trait's methods must all live in one `impl` block, so `client/mod.rs` keeps the
`impl CondoClient for HttpCondoClient` block and each domain module supplies inherent
`*_once` helpers that the trait methods delegate to. This is the pattern every later domain
follows.

Move into `condo-api/src/client/library.rs`: the `list_folder_once`, `file_meta_once`, and
`download_file_once` methods (wrapped in an inherent `impl HttpCondoClient` block), the
free function `parse_content_disposition_filename`, and the tests
`list_folder_parses_entries`, `list_folder_302_to_login_is_auth_error`,
`file_meta_reads_length_and_filename`, `download_file_writes_all_bytes`, and
`list_folder_reauths_once_then_gives_up`.

```rust
use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_file_list, Entry, FileMeta};
use std::io::Write;

impl HttpCondoClient {
    pub(crate) fn list_folder_once(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
        let resp = self
            .http
            .get(self.url("/library/get-file-list"))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(&[
                ("mode", "0".to_string()),
                ("folderID", folder_id.to_string()),
                ("searchString", String::new()),
                ("fileTypeSelectID", "0".to_string()),
                ("startDate", String::new()),
                ("endDate", String::new()),
                ("newSearch", "False".to_string()),
            ])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let text = resp.error_for_status()?.text()?;
        Ok(parse_file_list(&text)?)
    }

    pub(crate) fn file_meta_once(&self, file_id: u64) -> Result<FileMeta, ClientError> {
        // GET but read only the headers; drop the response without consuming the body.
        let resp = self
            .http
            .get(self.url("/library/download-file"))
            .query(&[("fileRecordID", file_id.to_string())])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let resp = resp.error_for_status()?;
        let size = resp.content_length().unwrap_or(0);
        let filename = resp
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_disposition_filename);
        // resp dropped here without reading the body.
        Ok(FileMeta { size, filename })
    }

    pub(crate) fn download_file_once(
        &self,
        file_id: u64,
        out: &mut dyn Write,
    ) -> Result<u64, ClientError> {
        let resp = self
            .http
            .get(self.url("/library/download-file"))
            .query(&[("fileRecordID", file_id.to_string())])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let mut resp = resp.error_for_status()?;
        let n = resp.copy_to(out)?;
        Ok(n)
    }
}

fn parse_content_disposition_filename(header: &str) -> Option<String> {
    // e.g. attachment; filename="01/09/25 Board Minutes.pdf"
    let idx = header.to_ascii_lowercase().find("filename=")?;
    let rest = &header[idx + "filename=".len()..];
    let rest = rest.trim();
    let name = rest
        .strip_prefix('"')
        .and_then(|s| s.split('"').next())
        .unwrap_or(rest);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}
```

The moved tests need `use crate::client::tests::creds;` — so in `client/mod.rs`, mark the
test module `pub(crate) mod tests` and its `creds()` helper `pub(crate) fn creds()`.

- [ ] **Step 8: Fix `condo-api/src/client/mod.rs`**

Remove the `ClientError` definition (now in `error.rs`), the three `*_once` methods, and
`parse_content_disposition_filename` (now in `library.rs`). Add `pub mod library;` at the
top. The struct fields `http`, `base_url`, and `creds` must become `pub(crate)` so
`library.rs` can reach them, and `url` and `with_reauth` likewise. Keep `login`, the
`CondoClient` trait, and the `impl CondoClient for HttpCondoClient` block delegating to the
`*_once` helpers exactly as they do today.

```rust
pub mod library;

use crate::credentials::Credentials;
use crate::error::ClientError;
use crate::model::{Entry, FileMeta};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::io::Write;

pub trait CondoClient {
    fn login(&self) -> Result<(), ClientError>;
    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError>;
    fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError>;
    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError>;
}

pub struct HttpCondoClient {
    pub(crate) http: Client,
    pub(crate) base_url: String,
    pub(crate) creds: Credentials,
}

impl HttpCondoClient {
    pub fn new(
        base_url: impl Into<String>,
        creds: Credentials,
    ) -> Result<HttpCondoClient, ClientError> {
        let http = Client::builder()
            .cookie_store(true)
            .redirect(Policy::none()) // we must see 302s to detect auth state
            .user_agent("condo-api/0.1")
            .build()?;
        Ok(HttpCondoClient {
            http,
            base_url: base_url.into(),
            creds,
        })
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Run `op`; if it fails with an expired session, re-authenticate once and retry.
    pub(crate) fn with_reauth<T>(
        &self,
        op: impl Fn() -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        match op() {
            Err(ClientError::Auth) => {
                log::info!("session expired; re-authenticating");
                self.login()?;
                op()
            }
            other => other,
        }
    }
}
```

The `impl CondoClient for HttpCondoClient` block and the `login` body carry over unchanged
from the current file.

- [ ] **Step 9: Write `condo-api/src/lib.rs`**

Flat re-exports at the crate root keep consumer imports to a single line.

```rust
pub mod client;
pub mod credentials;
pub mod error;
pub mod model;
pub mod paths;

pub use client::{CondoClient, HttpCondoClient};
pub use credentials::{Credentials, CredentialsError};
pub use error::ClientError;
pub use model::{parse_file_list, Entry, FileMeta};
```

- [ ] **Step 10: Write `condo-fs/Cargo.toml`**

```toml
[package]
name = "condo-fs"
description = "Mount a Condo Control File Library as a read-only FUSE filesystem"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lib]
name = "condo_fs"
path = "src/lib.rs"

[[bin]]
name = "condo-fs"
path = "src/bin/condo-fs.rs"

[dependencies]
condo-api = { path = "../condo-api" }
fuser = { version = "0.15", default-features = false }
libc = "0.2"
clap = { workspace = true }
log = { workspace = true }
env_logger = { workspace = true }
dirs = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 11: Fix the `condo-fs` imports**

Five edits, all mechanical:

`condo-fs/src/lib.rs` — drop the moved modules:

```rust
pub mod cache;
pub mod config;
pub mod fs;
pub mod names;
pub mod vfs;
```

`condo-fs/src/vfs.rs` lines 1-4 become:

```rust
use crate::cache::ContentCache;
use crate::names;
use condo_api::{ClientError, CondoClient, Entry};
```

`condo-fs/src/vfs.rs:337` (inside `mod tests`) becomes `use condo_api::FileMeta;`

`condo-fs/src/fs.rs:1` becomes `use condo_api::CondoClient;`

`condo-fs/src/config.rs` — delete the `expand_tilde` function and add
`use condo_api::paths::expand_tilde;`

`condo-fs/src/bin/condo-fs.rs` lines 2-7 become:

```rust
use condo_api::{CondoClient, Credentials, HttpCondoClient};
use condo_fs::cache::ContentCache;
use condo_fs::config::{Cli, Command, MountArgs};
use condo_fs::fs::CondoFs;
use condo_fs::vfs::Vfs;
```

- [ ] **Step 12: Fix `condo-api/tests/live_smoke.rs`**

Lines 3-5 become:

```rust
use condo_api::{CondoClient, Credentials, Entry, HttpCondoClient};
```

- [ ] **Step 13: Run the full suite — every existing test must still pass**

Run: `cargo test --all`
Expected: PASS. Specifically these must all appear and pass, unchanged in name and
assertion: `parses_username_and_password_with_special_chars`, `debug_redacts_password`,
`missing_password_is_an_error`, `parses_folder_rows`, `options_zero_folders_are_still_folders`,
`parses_file_rows_keeping_raw_name`, `login_success_sets_cookie_and_returns_ok`,
`login_failure_returns_auth_error`, `list_folder_parses_entries`,
`list_folder_302_to_login_is_auth_error`, `file_meta_reads_length_and_filename`,
`download_file_writes_all_bytes`, `list_folder_reauths_once_then_gives_up`.

If any test *disappeared* rather than failed, a test module was dropped during the move —
find it and restore it. A refactor that silently loses tests is a failed refactor.

- [ ] **Step 14: Run fmt and clippy**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings. The likely complaints are unused imports left behind in
`client/mod.rs` — remove them rather than adding `#[allow]`. Note the current
`#[allow(unused_imports)]` on the `model` import in `client.rs` should be **deleted**, not
carried over; after the split the import is genuinely used.

- [ ] **Step 15: Update the README build/layout references**

Search `README.md` for `src/`, `condo_fs::client`, `condo_fs::credentials`, and
`HttpCondoClient::new` (line ~201) and update them to the workspace layout and
`condo_api::` paths. Add a short "Workspace layout" section listing the three crates and
one sentence each on what they own (`condo-mcp` described as forthcoming).

- [ ] **Step 16: Commit**

```bash
git add -A
git commit -m "refactor: extract condo-api crate and convert repo to a workspace

Moves credentials, the HTTP client, and response models into a shared
condo-api crate so condo-fs and the forthcoming condo-mcp share one
login, session, and re-auth implementation. Client methods split by
domain (client/library.rs) to establish the pattern for later domains.
No behavior change; the existing test suite passes unmodified."
```

---

### Task 2: GET-only live probe for endpoint discovery

The announcements and calendar endpoints are unknown. This tool finds them by fetching
candidate paths from the live site with the user's own credentials and dumping the raw
responses to disk for inspection.

Two safety properties are **structural**, not conventional:
1. The only request-issuing code path is `reqwest`'s `.get()`. There is no parameter, flag,
   or branch that can produce another method.
2. Paths come from the command line or an explicit list file. Nothing is crawled, and no
   path is derived from a response body.

**Files:**
- Create: `condo-api/src/bin/condo-probe.rs`, `condo-api/tests/fixtures/probe_page.html`
- Modify: `condo-api/Cargo.toml`, `condo-api/src/client/mod.rs`, `condo-api/src/lib.rs`,
  `.gitignore`
- Test: tests live in `condo-api/src/client/mod.rs` (for `get_raw`) and in
  `condo-api/src/bin/condo-probe.rs` (for `safe_filename`)

**Interfaces:**
- Consumes: `HttpCondoClient::new`, `CondoClient::login`, `ClientError` from Task 1.
- Produces:
  - `condo_api::client::RawResponse { status: u16, final_url: String, headers: Vec<(String, String)>, body: String }`
  - `HttpCondoClient::get_raw(&self, path: &str, xhr: bool) -> Result<RawResponse, ClientError>`
    — inherent, **not** on the `CondoClient` trait, so the abstract client surface stays
    domain-shaped.

- [ ] **Step 1: Write the failing test for `get_raw`**

Add to the `tests` module in `condo-api/src/client/mod.rs`:

```rust
#[test]
fn get_raw_captures_status_headers_and_body() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/announcements/get-announcement-list")
            .header("x-requested-with", "XMLHttpRequest");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"[{"ID":1}]"#);
    });
    let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
    let raw = client
        .get_raw("/announcements/get-announcement-list", true)
        .unwrap();
    m.assert();
    assert_eq!(raw.status, 200);
    assert_eq!(raw.body, r#"[{"ID":1}]"#);
    assert!(raw
        .headers
        .iter()
        .any(|(k, v)| k == "content-type" && v.contains("application/json")));
}

#[test]
fn get_raw_reports_redirect_without_following_it() {
    // A 302 to /login means "not authenticated" — the probe must SEE it, not follow it,
    // so an unauthenticated probe run is obvious in the dumped output.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/calendar/get-event-list");
        then.status(302).header("location", "/login?NextPage=x");
    });
    let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
    let raw = client.get_raw("/calendar/get-event-list", false).unwrap();
    assert_eq!(raw.status, 302);
    assert!(raw
        .headers
        .iter()
        .any(|(k, v)| k == "location" && v.starts_with("/login")));
}

#[test]
fn get_raw_omits_xhr_header_when_not_requested() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/announcements")
            .matches(|req| {
                !req.headers
                    .as_ref()
                    .map(|h| h.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-requested-with")))
                    .unwrap_or(false)
            });
        then.status(200).body("<html></html>");
    });
    let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
    let raw = client.get_raw("/announcements", false).unwrap();
    m.assert();
    assert_eq!(raw.body, "<html></html>");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p condo-api get_raw`
Expected: FAIL — `no method named 'get_raw' found for struct 'HttpCondoClient'`.

- [ ] **Step 3: Implement `RawResponse` and `get_raw`**

Add to `condo-api/src/client/mod.rs`:

```rust
/// A verbatim capture of one GET response, for endpoint discovery.
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub final_url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpCondoClient {
    /// Fetch `path` and capture the response verbatim.
    ///
    /// GET only, by construction — there is no code path here that issues another method.
    /// Redirects are surfaced, not followed, so an expired session is visible in the
    /// capture rather than silently yielding a login page.
    ///
    /// `xhr` adds the `X-Requested-With: XMLHttpRequest` header that Condo Control's JSON
    /// endpoints require.
    pub fn get_raw(&self, path: &str, xhr: bool) -> Result<RawResponse, ClientError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            self.url(path)
        };
        let mut req = self.http.get(&url);
        if xhr {
            req = req.header("X-Requested-With", "XMLHttpRequest");
        }
        let resp = req.send()?;
        let status = resp.status().as_u16();
        let final_url = resp.url().to_string();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or("<non-utf8>").to_string(),
                )
            })
            .collect();
        let body = resp.text().unwrap_or_default();
        Ok(RawResponse {
            status,
            final_url,
            headers,
            body,
        })
    }
}
```

Re-export it in `condo-api/src/lib.rs` by extending the client re-export line:

```rust
pub use client::{CondoClient, HttpCondoClient, RawResponse};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p condo-api get_raw`
Expected: PASS, 3 tests.

- [ ] **Step 5: Add the `probe` feature and binary entry to `condo-api/Cargo.toml`**

The probe needs `clap`, `env_logger`, and `dirs` — dependencies the library itself does not
want. Gate them behind an optional feature so a normal `condo-api` build stays lean, and
mark the binary as requiring it.

```toml
[features]
probe = ["dep:clap", "dep:env_logger"]

[[bin]]
name = "condo-probe"
path = "src/bin/condo-probe.rs"
required-features = ["probe"]
```

and in `[dependencies]`, make the two new ones optional:

```toml
clap = { workspace = true, optional = true }
env_logger = { workspace = true, optional = true }
```

(`dirs` is already a hard dependency — `paths::expand_tilde` needs it.)

- [ ] **Step 6: Write the failing test for `safe_filename`**

Response dumps are named after the path they came from, so the output directory is
browsable. Path characters that are illegal or awkward in filenames get folded away.

Add this test module at the bottom of `condo-api/src/bin/condo-probe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::safe_filename;

    #[test]
    fn folds_path_separators_and_query_punctuation() {
        assert_eq!(
            safe_filename("/library/get-file-list?mode=0&folderID=137473"),
            "library_get-file-list_mode-0_folderID-137473"
        );
    }

    #[test]
    fn strips_leading_and_trailing_separators() {
        assert_eq!(safe_filename("/announcements/"), "announcements");
    }

    #[test]
    fn truncates_very_long_paths() {
        let long = format!("/x/{}", "a".repeat(300));
        let name = safe_filename(&long);
        assert!(name.len() <= 120, "got {} chars", name.len());
        assert!(name.starts_with("x_aaa"));
    }

    #[test]
    fn never_produces_a_path_traversal() {
        // A path from a list file must never escape the output directory.
        let name = safe_filename("/../../etc/passwd");
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }
}
```

- [ ] **Step 7: Run the test to verify it fails**

Run: `cargo test -p condo-api --features probe --bin condo-probe`
Expected: FAIL — the file does not exist yet, or `safe_filename` is undefined.

- [ ] **Step 8: Write the probe binary**

Create `condo-api/src/bin/condo-probe.rs`:

```rust
//! GET-only exploration tool for mapping undocumented Condo Control endpoints.
//!
//! Run with an explicit list of paths; nothing is crawled and no path is ever derived
//! from a response body. Every response is written verbatim to `--out-dir` for
//! inspection.
//!
//!   cargo run -p condo-api --features probe --bin condo-probe -- \
//!       --xhr /announcements/get-announcement-list /calendar/get-event-list
use clap::Parser;
use condo_api::paths::expand_tilde;
use condo_api::{CondoClient, Credentials, HttpCondoClient};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "condo-probe",
    about = "Fetch Condo Control paths (GET only) and dump the raw responses"
)]
struct Args {
    /// Paths to fetch, e.g. /announcements/get-announcement-list
    #[arg(required = true)]
    paths: Vec<String>,

    /// Credentials file (KEY=VALUE lines: USERNAME, PASSWORD)
    #[arg(long, default_value = "~/tokens/condo-control.txt")]
    credentials: String,

    #[arg(long, default_value = "https://app.condocontrol.com")]
    base_url: String,

    /// Where to write the response dumps
    #[arg(long, default_value = "probe-out")]
    out_dir: PathBuf,

    /// Send X-Requested-With: XMLHttpRequest (needed by the JSON endpoints)
    #[arg(long)]
    xhr: bool,

    /// Milliseconds to wait between requests
    #[arg(long, default_value_t = 1000)]
    delay_ms: u64,
}

/// Fold a URL path into a filename that is safe, readable, and confined to one directory.
fn safe_filename(path: &str) -> String {
    let mut out = String::new();
    for ch in path.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => out.push(ch),
            '=' => out.push('-'),
            _ => {
                if !out.ends_with('_') {
                    out.push('_');
                }
            }
        }
    }
    let trimmed = out.trim_matches('_');
    let mut name: String = trimmed.chars().take(120).collect();
    if name.is_empty() {
        name.push_str("root");
    }
    name
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let creds = Credentials::from_file(&expand_tilde(&args.credentials))?;
    let client = HttpCondoClient::new(&args.base_url, creds)?;
    log::info!("authenticating…");
    client.login()?;

    std::fs::create_dir_all(&args.out_dir)?;

    for (i, path) in args.paths.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(args.delay_ms));
        }
        log::info!("GET {path}");
        let raw = match client.get_raw(path, args.xhr) {
            Ok(raw) => raw,
            Err(e) => {
                log::warn!("{path}: {e}");
                continue;
            }
        };
        let out_path = args.out_dir.join(format!("{}.txt", safe_filename(path)));
        let mut f = std::fs::File::create(&out_path)?;
        writeln!(f, "GET {}", raw.final_url)?;
        writeln!(f, "status: {}", raw.status)?;
        for (k, v) in &raw.headers {
            writeln!(f, "{k}: {v}")?;
        }
        writeln!(f)?;
        write!(f, "{}", raw.body)?;
        log::info!(
            "  {} → {} ({} bytes) → {}",
            raw.status,
            raw.headers
                .iter()
                .find(|(k, _)| k == "content-type")
                .map(|(_, v)| v.as_str())
                .unwrap_or("?"),
            raw.body.len(),
            out_path.display()
        );
        if raw.status == 302 {
            log::warn!("  redirect — session may not be authenticated for this area");
        }
    }
    Ok(())
}
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p condo-api --features probe --bin condo-probe`
Expected: PASS, 4 tests.

- [ ] **Step 10: Keep probe output out of git**

Response dumps contain real resident data. Append to `.gitignore`:

```
/probe-out/
```

Verify: `printf '/probe-out/\n' >> .gitignore && git check-ignore -v probe-out/x.txt`
Expected: prints a matching `.gitignore` rule. If it prints nothing, the rule did not take
and dumps could be committed — fix before continuing.

- [ ] **Step 11: Run fmt, clippy, and the full suite**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: clean. Note `--all-features` builds the probe binary, so clippy covers it.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "feat(condo-api): add GET-only condo-probe for endpoint discovery

Adds HttpCondoClient::get_raw, which captures a response verbatim without
following redirects, and a feature-gated condo-probe binary that fetches
an explicit list of paths with a delay between them and dumps each
response for inspection. GET is the only method reachable from either.
Probe output is gitignored; it contains real resident data."
```

---

### Task 3: `condo-mcp` server skeleton with the `list_folder` tool

Stands up the MCP server end to end: stdio transport, one shared authenticated session, a
tool that calls the blocking client from async context, and error mapping written for an
agent to act on.

`list_folder` is the tool because it is the only domain already implemented and tested. It
proves the plumbing against a real endpoint instead of a placeholder.

**Files:**
- Create: `condo-mcp/Cargo.toml`, `condo-mcp/src/main.rs`, `condo-mcp/src/lib.rs`,
  `condo-mcp/src/server.rs`, `condo-mcp/src/tools/mod.rs`, `condo-mcp/src/tools/library.rs`
- Test: `condo-mcp/tests/server.rs`
- Modify: `Cargo.toml` (root — add the member), `README.md`

**Interfaces:**
- Consumes from Task 1: `condo_api::{CondoClient, HttpCondoClient, Credentials, ClientError, Entry}`, `condo_api::paths::expand_tilde`.
- Produces:
  - `condo_mcp::Args { credentials: String, base_url: String, root: u64 }` (clap `Parser`)
  - `condo_mcp::server::CondoMcp::new(base_url: impl Into<String>, creds: Credentials, root: u64, creds_path: String) -> Result<CondoMcp, ClientError>`
  - `CondoMcp::from_args(args: &Args) -> anyhow::Result<CondoMcp>`
  - `CondoMcp::with_client<T, F>(&self, f: F) -> Result<T, ClientError>` where `F: FnOnce(&HttpCondoClient) -> Result<T, ClientError> + Send + 'static`, `T: Send + 'static`
  - MCP tool `list_folder`, parameters `{ folder_id?: u64 }`, returning `Vec<FolderEntry>` where `FolderEntry { kind: String, id: u64, name: String, modified: Option<String> }`

- [ ] **Step 1: Create the crate manifest and register the workspace member**

`condo-mcp/Cargo.toml`:

```toml
[package]
name = "condo-mcp"
description = "Read-only MCP server exposing a Condo Control association"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[lib]
name = "condo_mcp"
path = "src/lib.rs"

[[bin]]
name = "condo-mcp"
path = "src/main.rs"

[dependencies]
condo-api = { path = "../condo-api" }
rmcp = { version = "3", features = ["server", "macros", "transport-io"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std", "io-util", "sync"] }
schemars = "1"
serde = { workspace = true }
serde_json = { workspace = true }
clap = { workspace = true }
anyhow = "1"
log = { workspace = true }
env_logger = { workspace = true }

[dev-dependencies]
httpmock = { workspace = true }
rmcp = { version = "3", features = ["client"] }
```

In the root `Cargo.toml`, change the members line to:

```toml
members = ["condo-api", "condo-fs", "condo-mcp"]
```

- [ ] **Step 2: Write the failing integration test**

Create `condo-mcp/tests/server.rs`. It runs a real MCP client against a real server over an
in-memory duplex pipe, with `httpmock` standing in for Condo Control — so it exercises the
whole path: tool dispatch, `spawn_blocking`, lazy login, client call, and serialization.

```rust
use condo_api::Credentials;
use condo_mcp::server::CondoMcp;
use httpmock::prelude::*;
use rmcp::model::CallToolRequestParam;
use rmcp::ServiceExt;

fn creds() -> Credentials {
    Credentials {
        username: "u@e.com".into(),
        password: "p@ss^#1".into(),
    }
}

/// Mock the login handshake: GET /login for a cookie, then a 302 away from /login.
fn mock_login(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/login");
        then.status(200)
            .header("set-cookie", "ASP.NET_SessionId=abc; path=/");
    });
    server.mock(|when, then| {
        when.method(POST).path("/login/login-post");
        then.status(302).header("location", "/my/my-home");
    });
}

async fn connect(server: &MockServer, root: u64) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let mcp = CondoMcp::new(server.base_url(), creds(), root, "<test>".into()).unwrap();
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let running = mcp.serve(server_side).await.unwrap();
        let _ = running.waiting().await;
    });
    ().serve(client_side).await.unwrap()
}

#[tokio::test]
async fn list_folder_tool_is_advertised() {
    let server = MockServer::start();
    mock_login(&server);
    let client = connect(&server, 137473).await;

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"list_folder"), "got tools: {names:?}");

    let t = tools.iter().find(|t| t.name.as_ref() == "list_folder").unwrap();
    assert!(
        t.description.as_ref().is_some_and(|d| !d.is_empty()),
        "every tool needs a description an agent can act on"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_folder_returns_entries_from_the_library() {
    let server = MockServer::start();
    mock_login(&server);
    let body = std::fs::read_to_string("../condo-api/tests/fixtures/files.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/library/get-file-list")
            .query_param("folderID", "262667");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_folder".into(),
            arguments: serde_json::json!({ "folder_id": 262667 }).as_object().cloned(),
        })
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool returned an error: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let entries = json.as_array().expect("structured content should be an array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["id"], 5369528);
    assert_eq!(entries[0]["kind"], "file");
    assert_eq!(entries[0]["name"], "01/09/25 Board Minutes");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn omitting_folder_id_uses_the_configured_root() {
    let server = MockServer::start();
    mock_login(&server);
    let body = std::fs::read_to_string("../condo-api/tests/fixtures/folders.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/library/get-file-list")
            .query_param("folderID", "137473");
        then.status(200).body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_folder".into(),
            arguments: None,
        })
        .await
        .unwrap();

    m.assert();
    assert_ne!(result.is_error, Some(true));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_empty_folder_returns_an_empty_list_not_an_error() {
    let server = MockServer::start();
    mock_login(&server);
    server.mock(|when, then| {
        when.method(GET).path("/library/get-file-list");
        then.status(200)
            .header("content-type", "application/json")
            .body("[]");
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_folder".into(),
            arguments: None,
        })
        .await
        .unwrap();

    assert_ne!(
        result.is_error,
        Some(true),
        "an empty folder is a normal result, not a failure"
    );
    let json = serde_json::to_value(&result.structured_content).unwrap();
    assert_eq!(json.as_array().map(|a| a.len()), Some(0));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_failing_tool_call_does_not_kill_the_server() {
    let server = MockServer::start();
    mock_login(&server);
    // Every listing 302s to /login: auth fails even after the one permitted retry.
    server.mock(|when, then| {
        when.method(GET).path("/library/get-file-list");
        then.status(302).header("location", "/login");
    });
    let client = connect(&server, 137473).await;

    let first = client
        .call_tool(CallToolRequestParam {
            name: "list_folder".into(),
            arguments: None,
        })
        .await;
    assert!(first.is_err() || first.unwrap().is_error == Some(true));

    // The server must still be answering after a tool failure.
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty(), "server died after a failing tool call");
    client.cancel().await.unwrap();
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p condo-mcp`
Expected: FAIL to compile — `condo_mcp::server` does not exist.

- [ ] **Step 4: Write `condo-mcp/src/server.rs`**

```rust
use crate::Args;
use condo_api::paths::expand_tilde;
use condo_api::{ClientError, CondoClient, Credentials, HttpCondoClient};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::ErrorData;
use rmcp::tool_router;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct CondoMcp {
    pub(crate) client: Arc<HttpCondoClient>,
    /// Guards the first login. Session *expiry* is handled inside the client by
    /// `with_reauth`; this only prevents every early tool call from logging in again.
    logged_in: Arc<Mutex<bool>>,
    pub(crate) root: u64,
    /// Shown in auth errors so the user knows which file to fix.
    creds_path: String,
    pub(crate) tool_router: ToolRouter<Self>,
}

impl CondoMcp {
    pub fn new(
        base_url: impl Into<String>,
        creds: Credentials,
        root: u64,
        creds_path: String,
    ) -> Result<CondoMcp, ClientError> {
        Ok(CondoMcp {
            client: Arc::new(HttpCondoClient::new(base_url, creds)?),
            logged_in: Arc::new(Mutex::new(false)),
            root,
            creds_path,
            tool_router: Self::tool_router(),
        })
    }

    pub fn from_args(args: &Args) -> anyhow::Result<CondoMcp> {
        let path = expand_tilde(&args.credentials);
        let creds = Credentials::from_file(&path)?;
        Ok(CondoMcp::new(
            args.base_url.clone(),
            creds,
            args.root,
            path.display().to_string(),
        )?)
    }

    /// Run a blocking client call off the async runtime, logging in first if needed.
    pub async fn with_client<T, F>(&self, f: F) -> Result<T, ClientError>
    where
        F: FnOnce(&HttpCondoClient) -> Result<T, ClientError> + Send + 'static,
        T: Send + 'static,
    {
        let client = self.client.clone();
        let logged_in = self.logged_in.clone();
        tokio::task::spawn_blocking(move || {
            {
                let mut done = logged_in.lock().expect("login mutex poisoned");
                if !*done {
                    client.login()?;
                    *done = true;
                }
            }
            f(&client)
        })
        .await
        .expect("blocking client task panicked")
    }

    /// Turn a client error into a message an agent can act on.
    pub(crate) fn tool_error(&self, e: ClientError) -> ErrorData {
        let msg = match e {
            ClientError::Auth => format!(
                "Authentication failed. Check the credentials file at {}.",
                self.creds_path
            ),
            ClientError::NotFound => "No such item in Condo Control.".to_string(),
            ClientError::Parse(ref inner) => {
                log::debug!("parse failure: {inner}");
                "Unexpected response shape from Condo Control; the site may have changed."
                    .to_string()
            }
            ClientError::Http(ref inner) => {
                format!("Condo Control is unreachable ({inner}). This may be worth retrying.")
            }
            ClientError::Io(ref inner) => format!("Local I/O error: {inner}"),
        };
        ErrorData::internal_error(msg, None)
    }

    /// Merge the per-domain routers. Each domain owns its own file and its own router;
    /// this is the one place they come together, and the only line a new domain touches.
    fn build_router() -> ToolRouter<Self> {
        Self::library_router()
    }
}
```

`build_router` will not compile until Step 5 defines `library_router`. That is expected —
the crate compiles at the end of Step 5, not here.

If `rmcp` turns out to require every `#[tool]` method to sit in the same `impl` block as
the `#[tool_router]` attribute, collapse Step 5's `impl` block into this file and keep
`tools/library.rs` for the wire types and the `From<Entry>` conversion only. The
`list_folder_tool_is_advertised` test is what tells you which way it went.

- [ ] **Step 5: Write `condo-mcp/src/tools/library.rs`**

```rust
use crate::server::CondoMcp;
use condo_api::{CondoClient, Entry};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListFolderParams {
    /// Folder ID to list. Omit to list the top of the File Library.
    pub folder_id: Option<u64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FolderEntry {
    /// Either "folder" or "file".
    pub kind: String,
    /// Pass a folder's id back as `folder_id` to descend into it.
    pub id: u64,
    pub name: String,
    /// Upload timestamp as Condo Control reports it; files only.
    pub modified: Option<String>,
}

impl From<Entry> for FolderEntry {
    fn from(e: Entry) -> FolderEntry {
        match e {
            Entry::Folder { id, name } => FolderEntry {
                kind: "folder".into(),
                id,
                name,
                modified: None,
            },
            Entry::File { id, name, date, .. } => FolderEntry {
                kind: "file".into(),
                id,
                name,
                modified: Some(date),
            },
        }
    }
}

#[rmcp::tool_router(router = library_router, vis = pub)]
impl CondoMcp {
    #[tool(
        name = "list_folder",
        description = "List the folders and files directly inside one File Library folder of the \
                       condo association's document library. Omit folder_id to list the top of the \
                       library. Returns entries in the order Condo Control provides, which is not \
                       sorted. File contents cannot be read yet."
    )]
    pub async fn list_folder(
        &self,
        Parameters(params): Parameters<ListFolderParams>,
    ) -> Result<Json<Vec<FolderEntry>>, ErrorData> {
        let id = params.folder_id.unwrap_or(self.root);
        let entries = self
            .with_client(move |c| c.list_folder(id))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(entries.into_iter().map(FolderEntry::from).collect()))
    }
}
```

`condo-mcp/src/tools/mod.rs`:

```rust
pub mod library;
```

Then add the `ServerHandler` impl to the bottom of `server.rs`:

```rust
#[rmcp::tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for CondoMcp {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo {
            instructions: Some(
                "Read-only access to a Condo Control condominium association. All tools are \
                 queries; nothing here can post, submit, or change anything."
                    .into(),
            ),
            capabilities: rmcp::model::ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
```

and change `tool_router: Self::tool_router()` in `new` to `tool_router: Self::build_router()`.

- [ ] **Step 6: Write `condo-mcp/src/lib.rs`**

```rust
pub mod server;
pub mod tools;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "condo-mcp",
    about = "Read-only MCP server exposing a Condo Control association"
)]
pub struct Args {
    /// Credentials file (KEY=VALUE lines: USERNAME, PASSWORD)
    #[arg(long, default_value = "~/tokens/condo-control.txt")]
    pub credentials: String,

    #[arg(long, default_value = "https://app.condocontrol.com")]
    pub base_url: String,

    /// Root folder ID in the File Library
    #[arg(long, default_value_t = 137473)]
    pub root: u64,
}
```

- [ ] **Step 7: Write `condo-mcp/src/main.rs`**

```rust
use clap::Parser;
use condo_mcp::server::CondoMcp;
use condo_mcp::Args;
use rmcp::transport::stdio;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout is the MCP wire protocol. Logs MUST go to stderr or the server is unusable.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();

    let args = Args::parse();
    // Login is deliberately lazy — the first tool call authenticates. Failing at startup
    // over a missing credentials file looks identical to a broken install in most clients.
    let server = CondoMcp::from_args(&args)?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p condo-mcp`
Expected: PASS, 5 tests.

If `list_folder_tool_is_advertised` fails with an empty tool list, the router merge in
Step 5 is not wired up — that is the failure mode this test exists to catch. Fix the router
rather than the test.

- [ ] **Step 9: Verify no tool can write**

Run: `grep -rnE '\.(post|put|patch|delete)\(' condo-api/src condo-mcp/src`
Expected: exactly one hit — the login POST in `condo-api/src/client/mod.rs`. Any other hit
violates the read-only constraint and must be removed.

- [ ] **Step 10: Run fmt, clippy, and the full suite**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: clean.

- [ ] **Step 11: Verify the server actually speaks MCP over stdio**

The tests use a duplex pipe, which does not prove the stdio transport or the
stdout-is-sacred rule. Check by hand:

```bash
cargo build -p condo-mcp
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' \
  | ./target/debug/condo-mcp --credentials ~/tokens/condo-control.txt 2>/dev/null
```

Expected: a single line of JSON on stdout containing `"result"` and `"serverInfo"`, with no
log lines mixed in. If log output appears on stdout, the logger target is wrong and every
MCP client will fail to parse the stream.

- [ ] **Step 12: Document how to register the server**

Add a `condo-mcp` section to `README.md` covering what it is (read-only), the available
tool, and the client registration:

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

Or, for Claude Code: `claude mcp add condo -- /path/to/target/release/condo-mcp`

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "feat(condo-mcp): add read-only MCP server with the list_folder tool

Stdio server built on rmcp, sharing one authenticated blocking client
across tools via spawn_blocking and a lazily-established session. Ships
the list_folder tool over the existing File Library client, which proves
the transport, dispatch, and error-mapping paths end to end before the
unmapped domains are built. Logging is pinned to stderr because stdout
carries the protocol."
```

---

## Discovery checkpoint

After Task 3, run the probe against candidate endpoints for the two Phase 1 domains. This
is a manual, judgment-driven step — read what comes back before writing any parser.

```bash
cargo run -p condo-api --features probe --bin condo-probe -- \
  /announcements /calendar

cargo run -p condo-api --features probe --bin condo-probe -- --xhr \
  /announcements/get-announcement-list \
  /calendar/get-event-list
```

Then, from the HTML pages fetched in the first run, read out the actual XHR URLs the page
calls and probe those. A `302` to `/login` means that area rejects the session; a `404`
means the guess was wrong; a `200` with JSON is the target.

Produce from this:
1. Confirmed request shapes: path, query parameters, whether the XHR header is required.
2. Confirmed response shapes: field names and types, date formats, pagination if any.
3. Anonymized fixtures committed to `condo-api/tests/fixtures/` — resident names, unit
   numbers, phone numbers, and email addresses replaced with plausible fakes. **Real
   resident data must not enter git.**

With those in hand, write `docs/superpowers/plans/2026-08-10-condo-mcp-domains.md` covering
announcements and calendar, following the `client/library.rs` + `model/library.rs` +
`tools/library.rs` pattern this plan establishes.

---

## Follow-ups carried out of this plan

Raised by the final whole-branch review, deliberately not fixed here. The first three get
cheaper the sooner they are done and more expensive with every domain added — settle them in
the domains plan, not after.

**Trait shape.** `CondoClient` has four methods today. Six more domains at two to three
methods each makes it an ~18-method trait, and `condo-fs`'s hand-written `MockClient`
(`condo-fs/src/vfs.rs`) must implement every one of them — including the dozen it will never
call — or `condo-fs`'s tests stop compiling. Split into per-domain traits (`LibraryClient`,
`AnnouncementsClient`, …) that `HttpCondoClient` implements and consumers depend on à la
carte. Nearly free with one domain; painful at three.

**`names` is in the wrong crate.** `names::infer_extension`, `names::sanitize_name`, and
`names::parse_condo_date` live in `condo-fs` but are model-layer concerns. `condo-mcp` is
already the second consumer that wants them.

**`FolderEntry` drops `thumbnail`.** `condo-fs` derives the file type from it — which is why
the mount shows `Board Minutes.pdf` — so an MCP client currently cannot tell a PDF from a
spreadsheet. Add a `file_type` field once `names` moves.

**Structural read-only enforcement.** `HttpCondoClient.http` is `pub(crate)`, so any future
`client/<domain>.rs` can reach it and write `.post(...)`. `scripts/guardrails.sh` catches that
textually; making `http` private behind a `fn get(&self, path) -> RequestBuilder` helper would
make a non-GET request inexpressible rather than merely detectable.

**Guardrail script weaknesses** (parked, non-blocking): it fails open if a scanned directory
is renamed or removed — grep's "No such file" is swallowed and the check reports OK, so a
future restructure silently disables it while CI stays green (fix: a `[ -d ]` precondition per
directory). The login-POST exemption is line-scoped, so a violation sharing a physical line
with the exempt text escapes; that requires deliberately pasting the exempt string as a
comment, so it is not a realistic accident. It is also textual, so `request(Method::POST, …)`
is not detected — the codebase uses the builder form throughout.

**`tool_error` shape.** Every failure except `NotFound` is a JSON-RPC protocol error rather
than an `is_error: true` tool result; MCP guidance generally prefers the latter so the model
sees the failure as tool output. Revisit when the domain tools that make it matter exist.

**`CondoMcp::from_args`** inherits `new`'s nested-runtime panic and has no `# Panics` doc,
though it is the only path `main.rs` uses.
