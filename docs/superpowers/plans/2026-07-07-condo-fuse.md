# condo-fuse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mount a Condo Control association File Library subtree as a read-only FUSE filesystem, driven by a reusable, FUSE-agnostic API client.

**Architecture:** A single Rust library crate `condo-fuse` with a thin `condo-fuse` binary. A `CondoClient` trait (impl `HttpCondoClient` over blocking `reqwest`) knows how to log in, list folders, and download files. A `Vfs<C: CondoClient>` holds an inode table + caches and exposes plain, unit-testable methods (`lookup`/`getattr`/`readdir`/`open`/`read`). A thin `CondoFs` adapter implements `fuser::Filesystem` by delegating to `Vfs`. The client and Vfs are tested without network (httpmock + a mock client); the binary is validated by a real mount.

**Tech Stack:** Rust 2021, `fuser` 0.15 (default-features off → mounts via `fusermount3`, no libfuse-dev), `reqwest` 0.12 blocking (cookies+multipart), `serde`/`serde_json`, `clap` 4, `libc`, `thiserror`; dev: `httpmock` 0.7, `tempfile` 3.

## Global Constraints

- **Read-only.** No create/rename/move/delete/upload code paths anywhere. FUSE mounted with `MountOption::RO`.
- **Rust edition 2021**, toolchain ≥ 1.95 (installed).
- **`fuser = { version = "0.15", default-features = false }`** — verified to build without `libfuse-dev`; mounts through the installed `fusermount3`. Do NOT enable default features.
- **Credentials file keys are `USERNAME` and `PASSWORD`** (not `PASS`). Values may contain `^`, `#`, spaces — parse the file directly, split on the first `=`, trim only trailing `\r`/`\n`. Never pass through a shell.
- **Base URL is injectable** (`https://app.condocontrol.com` default) so tests point the client at a local mock server.
- **HTTP redirects disabled** (`redirect::Policy::none()`) so auth state is detectable: authed API calls return `200`; expired sessions return `302` to `/login`.
- **`X-Requested-With: XMLHttpRequest`** header on `get-file-list` requests.
- Default paths: credentials `~/tokens/condo-control.txt`, cache dir `~/.cache/condo-fuse`, root folder `137473`, `--meta-ttl` 60s.
- Every entry is read-only: dirs `0o555`, files `0o444`, owned by the mounting uid/gid.

---

## File Structure

- `Cargo.toml` — crate manifest, `[lib]` + `[[bin]]`.
- `src/lib.rs` — module declarations + re-exports.
- `src/credentials.rs` — `Credentials`, `from_file`.
- `src/model.rs` — `Entry`, `FileMeta` types.
- `src/names.rs` — `sanitize_name`, `infer_extension`, `parse_condo_date`, collision resolution.
- `src/client.rs` — `CondoClient` trait, `HttpCondoClient`.
- `src/cache.rs` — `ContentCache` (disk), `TtlCache<K,V>` (in-memory metadata).
- `src/vfs.rs` — `Vfs<C>`, `Node`, `VfsError`, inode table, `FileAttr` building.
- `src/fs.rs` — `CondoFs<C>` implementing `fuser::Filesystem`.
- `src/config.rs` — `Config`, `MountArgs` (clap).
- `src/bin/condo-fuse.rs` — CLI entry (`mount` subcommand).
- `tests/fixtures/*.json`, `tests/fixtures/credentials.txt` — recorded fixtures.

---

## Task 1: Project scaffold

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `src/bin/condo-fuse.rs`
- Create (empty stubs): `src/credentials.rs`, `src/model.rs`, `src/names.rs`, `src/client.rs`, `src/cache.rs`, `src/vfs.rs`, `src/fs.rs`, `src/config.rs`

**Interfaces:**
- Produces: a compiling library crate `condo_fuse` + binary `condo-fuse`.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "condo-fuse"
version = "0.1.0"
edition = "2021"
rust-version = "1.95"

[lib]
name = "condo_fuse"
path = "src/lib.rs"

[[bin]]
name = "condo-fuse"
path = "src/bin/condo-fuse.rs"

[dependencies]
fuser = { version = "0.15", default-features = false }
reqwest = { version = "0.12", default-features = false, features = ["blocking", "cookies", "multipart", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
libc = "0.2"
thiserror = "2"
log = "0.4"
env_logger = "0.11"
dirs = "5"

[dev-dependencies]
httpmock = "0.7"
tempfile = "3"
```

- [ ] **Step 2: Write `src/lib.rs`**

```rust
pub mod cache;
pub mod client;
pub mod config;
pub mod credentials;
pub mod fs;
pub mod model;
pub mod names;
pub mod vfs;
```

- [ ] **Step 3: Create empty module stubs**

Each of `src/credentials.rs`, `src/model.rs`, `src/names.rs`, `src/client.rs`, `src/cache.rs`, `src/vfs.rs`, `src/fs.rs`, `src/config.rs` starts empty (or `// placeholder`).

- [ ] **Step 4: Write minimal `src/bin/condo-fuse.rs`**

```rust
fn main() {
    println!("condo-fuse");
}
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles (warnings about unused modules are fine).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "scaffold condo-fuse crate"
```

---

## Task 2: Credentials parsing

**Files:**
- Modify: `src/credentials.rs`
- Create: `tests/fixtures/credentials.txt`

**Interfaces:**
- Produces: `pub struct Credentials { pub username: String, pub password: String }` and `impl Credentials { pub fn from_file(path: &std::path::Path) -> anyhow::Result<Credentials> }` — but use `thiserror`, not anyhow. Exact signature: `pub fn from_file(path: &std::path::Path) -> Result<Credentials, CredentialsError>`.

- [ ] **Step 1: Create the fixture `tests/fixtures/credentials.txt`**

Exactly these three lines (note special chars in the password and a trailing newline):

```
USERNAME=ryan@example.com
PASSWORD=p@ss^word#1 two
```

- [ ] **Step 2: Write the failing test in `src/credentials.rs`**

```rust
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("reading credentials file: {0}")]
    Io(#[from] std::io::Error),
    #[error("missing {0} in credentials file")]
    Missing(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_username_and_password_with_special_chars() {
        let c = Credentials::from_file(Path::new("tests/fixtures/credentials.txt")).unwrap();
        assert_eq!(c.username, "ryan@example.com");
        assert_eq!(c.password, "p@ss^word#1 two");
    }

    #[test]
    fn missing_password_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("creds.txt");
        std::fs::write(&p, "USERNAME=only@example.com\n").unwrap();
        assert!(matches!(Credentials::from_file(&p), Err(CredentialsError::Missing("PASSWORD"))));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --lib credentials`
Expected: FAIL — `from_file` not found.

- [ ] **Step 4: Implement `from_file`** (add above the `#[cfg(test)]` block)

```rust
impl Credentials {
    pub fn from_file(path: &Path) -> Result<Credentials, CredentialsError> {
        let contents = std::fs::read_to_string(path)?;
        let mut username = None;
        let mut password = None;
        for line in contents.lines() {
            let line = line.trim_end_matches(['\r', '\n']);
            let Some((key, value)) = line.split_once('=') else { continue };
            match key.trim().to_ascii_uppercase().as_str() {
                "USERNAME" => username = Some(value.to_string()),
                "PASSWORD" => password = Some(value.to_string()),
                _ => {}
            }
        }
        Ok(Credentials {
            username: username.ok_or(CredentialsError::Missing("USERNAME"))?,
            password: password.ok_or(CredentialsError::Missing("PASSWORD"))?,
        })
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib credentials`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/credentials.rs tests/fixtures/credentials.txt
git commit -m "add credentials file parsing"
```

---

## Task 3: Entry model + get-file-list JSON parsing

**Files:**
- Modify: `src/model.rs`
- Create: `tests/fixtures/folders.json`, `tests/fixtures/files.json`

**Interfaces:**
- Produces:
  - `pub enum Entry { Folder { id: u64, name: String }, File { id: u64, key: String, name: String, date: String, thumbnail: String } }`
  - `pub struct FileMeta { pub size: u64, pub filename: Option<String> }`
  - `pub fn parse_file_list(json: &str) -> Result<Vec<Entry>, serde_json::Error>`

- [ ] **Step 1: Create `tests/fixtures/folders.json`** (real shape captured from the API — folders have `Options: 2`)

```json
[
 {"ID":162100,"Key":"","Thumbnail":"/shared/images/icons/folder.gif","Name":"Board of Directors","Desc":null,"Date":"","Link":"https://app.condocontrol.com/library/view-folder?folderID=162100","Options":2},
 {"ID":140698,"Key":"","Thumbnail":"/shared/images/icons/folder.gif","Name":"Financial","Desc":"stmts","Date":"","Link":"https://app.condocontrol.com/library/view-folder?folderID=140698","Options":2}
]
```

- [ ] **Step 2: Create `tests/fixtures/files.json`** (files have `Options: 1`, a `Key`, a real `Date`, `/` in the name, a pdf thumbnail)

```json
[
 {"ID":5369528,"Key":"9E825A05-B799-4A3A-8635-9C9B19A66ADB","Thumbnail":"/shared/images/icons/pdf-128x128.png","Name":"01/09/25 Board Minutes","Desc":null,"Date":"2025-01-18 02:41:25","Link":"https://app.condocontrol.com/library/view-file.aspx?FileRecordID=5369528&Key=9E825A05","Options":1},
 {"ID":5454287,"Key":"6F46F72D-AF52-4ECA-9250-A7EE6DE7990E","Thumbnail":"/shared/images/icons/pdf-128x128.png","Name":"01/18/25 Board Minutes","Desc":null,"Date":"2025-02-06 06:31:51","Link":"https://app.condocontrol.com/library/view-file.aspx?FileRecordID=5454287&Key=6F46F72D","Options":1}
]
```

- [ ] **Step 3: Write the failing test in `src/model.rs`**

```rust
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Folder { id: u64, name: String },
    File { id: u64, key: String, name: String, date: String, thumbnail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub filename: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folder_rows() {
        let json = std::fs::read_to_string("tests/fixtures/folders.json").unwrap();
        let entries = parse_file_list(&json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], Entry::Folder { id: 162100, name: "Board of Directors".into() });
    }

    #[test]
    fn parses_file_rows_keeping_raw_name() {
        let json = std::fs::read_to_string("tests/fixtures/files.json").unwrap();
        let entries = parse_file_list(&json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            Entry::File {
                id: 5369528,
                key: "9E825A05-B799-4A3A-8635-9C9B19A66ADB".into(),
                name: "01/09/25 Board Minutes".into(),
                date: "2025-01-18 02:41:25".into(),
                thumbnail: "/shared/images/icons/pdf-128x128.png".into(),
            }
        );
    }
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test --lib model`
Expected: FAIL — `parse_file_list` not found.

- [ ] **Step 5: Implement `parse_file_list`** (add above `#[cfg(test)]`)

```rust
#[derive(Deserialize)]
struct RawRow {
    #[serde(rename = "ID")]
    id: u64,
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "Thumbnail")]
    thumbnail: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Date")]
    date: String,
    #[serde(rename = "Options")]
    options: u8,
}

pub fn parse_file_list(json: &str) -> Result<Vec<Entry>, serde_json::Error> {
    let rows: Vec<RawRow> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| match r.options {
            2 => Some(Entry::Folder { id: r.id, name: r.name }),
            1 => Some(Entry::File {
                id: r.id,
                key: r.key,
                name: r.name,
                date: r.date,
                thumbnail: r.thumbnail,
            }),
            _ => {
                log::warn!("skipping row {} with unknown Options={}", r.id, r.options);
                None
            }
        })
        .collect())
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib model`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
git add src/model.rs tests/fixtures/folders.json tests/fixtures/files.json
git commit -m "parse get-file-list into folder/file entries"
```

---

## Task 4: Names — sanitize, infer extension, parse date

**Files:**
- Modify: `src/names.rs`

**Interfaces:**
- Produces:
  - `pub fn sanitize_name(raw: &str) -> String` — replaces `/` with `-`, replaces NUL, trims surrounding whitespace, maps empty → `"_"`.
  - `pub fn infer_extension(thumbnail: &str) -> Option<&'static str>` — returns e.g. `"pdf"` for `pdf-128x128.png`.
  - `pub fn file_display_name(raw: &str, thumbnail: &str) -> String` — sanitized name with extension appended when inferred and not already present.
  - `pub fn resolve_collisions(names: Vec<String>) -> Vec<String>` — appends ` (2)`, ` (3)`… to duplicates in order (case-insensitive comparison; extension preserved).
  - `pub fn parse_condo_date(date: &str) -> std::time::SystemTime` — parses `YYYY-MM-DD HH:MM:SS` (UTC) → `SystemTime`; returns `UNIX_EPOCH` on any parse failure.

- [ ] **Step 1: Write the failing tests in `src/names.rs`**

```rust
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(sanitize_name("01/09/25 Board Minutes"), "01-09-25 Board Minutes");
        assert_eq!(sanitize_name("  spaced  "), "spaced");
        assert_eq!(sanitize_name("/"), "-");
        assert_eq!(sanitize_name(""), "_");
    }

    #[test]
    fn infers_extension_from_thumbnail() {
        assert_eq!(infer_extension("/shared/images/icons/pdf-128x128.png"), Some("pdf"));
        assert_eq!(infer_extension("/shared/images/icons/folder.gif"), None);
        assert_eq!(infer_extension("/x/doc-128x128.png"), Some("doc"));
    }

    #[test]
    fn file_display_name_appends_extension() {
        assert_eq!(
            file_display_name("01/09/25 Board Minutes", "/shared/images/icons/pdf-128x128.png"),
            "01-09-25 Board Minutes.pdf"
        );
        // does not double up if already present
        assert_eq!(
            file_display_name("report.pdf", "/shared/images/icons/pdf-128x128.png"),
            "report.pdf"
        );
        // unknown icon: no extension
        assert_eq!(file_display_name("thing", "/shared/images/icons/mystery.gif"), "thing");
    }

    #[test]
    fn resolves_duplicate_names() {
        let out = resolve_collisions(vec![
            "a.pdf".into(),
            "a.pdf".into(),
            "b".into(),
            "a.pdf".into(),
        ]);
        assert_eq!(out, vec!["a.pdf", "a (2).pdf", "b", "a (3).pdf"]);
    }

    #[test]
    fn parses_condo_date() {
        let t = parse_condo_date("2025-01-18 02:41:25");
        let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 1737168085); // 2025-01-18T02:41:25Z
        assert_eq!(parse_condo_date("garbage"), UNIX_EPOCH);
        assert_eq!(parse_condo_date(""), UNIX_EPOCH);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib names`
Expected: FAIL — functions not found.

- [ ] **Step 3: Implement the functions** (add above `#[cfg(test)]`)

```rust
pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '/' => '-',
            '\0' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Icons are named like `pdf-128x128.png`; the leading token is the type.
pub fn infer_extension(thumbnail: &str) -> Option<&'static str> {
    let file = thumbnail.rsplit('/').next().unwrap_or(thumbnail);
    let token = file.split(['-', '.']).next().unwrap_or("");
    match token {
        "pdf" => Some("pdf"),
        "doc" => Some("doc"),
        "docx" => Some("docx"),
        "xls" => Some("xls"),
        "xlsx" => Some("xlsx"),
        "ppt" => Some("ppt"),
        "pptx" => Some("pptx"),
        "txt" => Some("txt"),
        "csv" => Some("csv"),
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "gif" => None, // folder.gif and generic icons — never a real file type here
        "zip" => Some("zip"),
        _ => None,
    }
}

pub fn file_display_name(raw: &str, thumbnail: &str) -> String {
    let base = sanitize_name(raw);
    match infer_extension(thumbnail) {
        Some(ext) if !base.to_ascii_lowercase().ends_with(&format!(".{ext}")) => {
            format!("{base}.{ext}")
        }
        _ => base,
    }
}

pub fn resolve_collisions(names: Vec<String>) -> Vec<String> {
    use std::collections::HashMap;
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let key = name.to_ascii_lowercase();
        let count = seen.entry(key).or_insert(0);
        *count += 1;
        if *count == 1 {
            out.push(name);
        } else {
            // insert " (N)" before the extension if any
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            out.push(format!("{stem} ({count}){ext}"));
        }
    }
    out
}

pub fn parse_condo_date(date: &str) -> SystemTime {
    // Format: YYYY-MM-DD HH:MM:SS, treated as UTC.
    fn parse(date: &str) -> Option<u64> {
        let (d, t) = date.split_once(' ')?;
        let mut dp = d.split('-');
        let year: i64 = dp.next()?.parse().ok()?;
        let month: i64 = dp.next()?.parse().ok()?;
        let day: i64 = dp.next()?.parse().ok()?;
        let mut tp = t.split(':');
        let hh: i64 = tp.next()?.parse().ok()?;
        let mm: i64 = tp.next()?.parse().ok()?;
        let ss: i64 = tp.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        // days from civil (Howard Hinnant's algorithm)
        let y = if month <= 2 { year - 1 } else { year };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as i64;
        let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        let secs = days * 86400 + hh * 3600 + mm * 60 + ss;
        if secs < 0 { None } else { Some(secs as u64) }
    }
    match parse(date) {
        Some(secs) => UNIX_EPOCH + Duration::from_secs(secs),
        None => UNIX_EPOCH,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib names`
Expected: PASS (5 tests). If `parses_condo_date` disagrees on the epoch value, recompute with `date -u -d '2025-01-18 02:41:25' +%s` and update the assertion to that number.

- [ ] **Step 5: Commit**

```bash
git add src/names.rs
git commit -m "add name sanitization, extension inference, date parsing"
```

---

## Task 5: CondoClient trait + login

**Files:**
- Modify: `src/client.rs`

**Interfaces:**
- Consumes: `crate::credentials::Credentials`, `crate::model::{Entry, FileMeta, parse_file_list}`.
- Produces:
  - `pub trait CondoClient { fn login(&self) -> Result<(), ClientError>; fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError>; fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError>; fn download_file(&self, file_id: u64, out: &mut dyn std::io::Write) -> Result<u64, ClientError>; }`
  - `pub struct HttpCondoClient { /* http, base_url, creds */ }` with `pub fn new(base_url: impl Into<String>, creds: Credentials) -> Result<HttpCondoClient, ClientError>`.
  - `pub enum ClientError` (thiserror): `Http(reqwest::Error)`, `Auth`, `Parse(serde_json::Error)`, `Io(std::io::Error)`, `NotFound`.

- [ ] **Step 1: Write the failing login test in `src/client.rs`**

```rust
use crate::credentials::Credentials;
use crate::model::{parse_file_list, Entry, FileMeta};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::io::Write;
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

pub trait CondoClient {
    fn login(&self) -> Result<(), ClientError>;
    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError>;
    fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError>;
    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn creds() -> Credentials {
        Credentials { username: "u@e.com".into(), password: "p@ss^#1".into() }
    }

    #[test]
    fn login_success_sets_cookie_and_returns_ok() {
        let server = MockServer::start();
        let get_login = server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200).header("set-cookie", "ASP.NET_SessionId=abc; path=/").body("<form/>");
        });
        let post_login = server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home")
                .header("set-cookie", "CCCookie=xyz; path=/");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        client.login().unwrap();
        get_login.assert();
        post_login.assert();
    }

    #[test]
    fn login_failure_returns_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| { when.method(GET).path("/login"); then.status(200); });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/login"); // bounce back = failure
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.login(), Err(ClientError::Auth)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib client::tests::login`
Expected: FAIL — `HttpCondoClient` not found.

- [ ] **Step 3: Implement `HttpCondoClient::new` + `login`** (add above `#[cfg(test)]`)

```rust
pub struct HttpCondoClient {
    http: Client,
    base_url: String,
    creds: Credentials,
}

impl HttpCondoClient {
    pub fn new(base_url: impl Into<String>, creds: Credentials) -> Result<HttpCondoClient, ClientError> {
        let http = Client::builder()
            .cookie_store(true)
            .redirect(Policy::none()) // we must see 302s to detect auth state
            .user_agent("condo-fuse/0.1")
            .build()?;
        Ok(HttpCondoClient { http, base_url: base_url.into(), creds })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }
}

impl CondoClient for HttpCondoClient {
    fn login(&self) -> Result<(), ClientError> {
        // 1. GET /login to obtain a session cookie.
        self.http.get(self.url("/login")).send()?;

        // 2. POST credentials as multipart/form-data.
        let form = reqwest::blocking::multipart::Form::new()
            .text("Username", self.creds.username.clone())
            .text("Password", self.creds.password.clone())
            .text("SaveEmail", "false")
            .text("Lang", "en")
            .text("RedirectURL", "");
        let resp = self.http.post(self.url("/login/login-post")).multipart(form).send()?;

        // Success = 302 to /my/... ; failure = 302 back to /login.
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if location.starts_with("/login") || location.contains("/login?") {
            return Err(ClientError::Auth);
        }
        Ok(())
    }

    fn list_folder(&self, _folder_id: u64) -> Result<Vec<Entry>, ClientError> {
        unimplemented!("Task 6")
    }
    fn file_meta(&self, _file_id: u64) -> Result<FileMeta, ClientError> {
        unimplemented!("Task 7")
    }
    fn download_file(&self, _file_id: u64, _out: &mut dyn Write) -> Result<u64, ClientError> {
        unimplemented!("Task 7")
    }
}
```

Note: `parse_file_list` import is unused until Task 6 — add `#[allow(unused_imports)]` on it or wire it now; the warning is harmless.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib client::tests::login`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/client.rs
git commit -m "add CondoClient trait and login via multipart form"
```

---

## Task 6: list_folder

**Files:**
- Modify: `src/client.rs`

**Interfaces:**
- Consumes: `parse_file_list`. Produces: working `list_folder` that GETs `/library/get-file-list` with the required params and the `X-Requested-With` header, returns `NotFound`-style empty handling via `Auth` on a `302`.

- [ ] **Step 1: Add the failing test to `src/client.rs` `tests` module**

```rust
    #[test]
    fn list_folder_parses_entries() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/files.json").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/library/get-file-list")
                .query_param("folderID", "262667")
                .query_param("mode", "0")
                .query_param("newSearch", "False")
                .header("x-requested-with", "XMLHttpRequest");
            then.status(200).header("content-type", "application/json").body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let entries = client.list_folder(262667).unwrap();
        m.assert();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], Entry::File { id: 5369528, .. }));
    }

    #[test]
    fn list_folder_302_to_login_is_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/library/get-file-list");
            then.status(302).header("location", "/login?NextPage=x");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.list_folder(1), Err(ClientError::Auth)));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib client::tests::list_folder`
Expected: FAIL — `unimplemented!`.

- [ ] **Step 3: Implement `list_folder`** (replace the stub body)

```rust
    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib client::tests::list_folder`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/client.rs
git commit -m "implement list_folder against get-file-list"
```

---

## Task 7: file_meta + download_file

**Files:**
- Modify: `src/client.rs`

**Interfaces:**
- Produces: `file_meta` (reads `Content-Length` + `Content-Disposition` without consuming the body) and `download_file` (streams full body to a writer, returns bytes written).

- [ ] **Step 1: Add failing tests to the `tests` module**

```rust
    #[test]
    fn file_meta_reads_length_and_filename() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/library/download-file").query_param("fileRecordID", "5369528");
            then.status(200)
                .header("content-length", "279033")
                .header("content-disposition", "attachment; filename=\"01/09/25 Board Minutes.pdf\"")
                .header("content-type", "application/pdf")
                .body(vec![0u8; 279033]);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let meta = client.file_meta(5369528).unwrap();
        m.assert();
        assert_eq!(meta.size, 279033);
        assert_eq!(meta.filename.as_deref(), Some("01/09/25 Board Minutes.pdf"));
    }

    #[test]
    fn download_file_writes_all_bytes() {
        let server = MockServer::start();
        let payload = b"%PDF-1.7 hello".to_vec();
        server.mock(|when, then| {
            when.method(GET).path("/library/download-file").query_param("fileRecordID", "42");
            then.status(200).body(payload.clone());
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let n = client.download_file(42, &mut buf).unwrap();
        assert_eq!(n as usize, payload.len());
        assert_eq!(buf, payload);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib client::tests::file_meta client::tests::download_file`
Expected: FAIL — `unimplemented!`.

- [ ] **Step 3: Implement both** (replace stubs)

```rust
    fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError> {
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

    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError> {
        let mut resp = self
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
```

And add this free function above `#[cfg(test)]`:

```rust
fn parse_content_disposition_filename(header: &str) -> Option<String> {
    // e.g. attachment; filename="01/09/25 Board Minutes.pdf"
    let idx = header.to_ascii_lowercase().find("filename=")?;
    let rest = &header[idx + "filename=".len()..];
    let rest = rest.trim();
    let name = rest.strip_prefix('"').and_then(|s| s.split('"').next()).unwrap_or(rest);
    if name.is_empty() { None } else { Some(name.to_string()) }
}
```

Note: the `mut resp` binding in `download_file` needs `resp` mutable; adjust the earlier `let mut resp` / `resp.error_for_status()` chain so the final binding is `let mut resp = ...error_for_status()?;` then `resp.copy_to(out)`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib client`
Expected: PASS (all client tests).

- [ ] **Step 5: Commit**

```bash
git add src/client.rs
git commit -m "add file_meta (header-only) and download_file"
```

---

## Task 8: Transparent re-auth

**Files:**
- Modify: `src/client.rs`

**Interfaces:**
- Produces: a private helper `fn with_reauth<T>(&self, op: impl Fn() -> Result<T, ClientError>) -> Result<T, ClientError>` that, on a first `ClientError::Auth`, calls `login()` once and retries `op`. `list_folder`/`file_meta`/`download_file` wrap their core in it.

- [ ] **Step 1: Add a failing test to the `tests` module**

```rust
    #[test]
    fn list_folder_reauths_on_expired_session() {
        let server = MockServer::start();
        // login endpoints
        server.mock(|when, then| { when.method(GET).path("/login"); then.status(200); });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        // First get-file-list call: expired -> 302 /login. Second: success.
        // httpmock matches in insertion order with `.times`; use a hits-based toggle:
        let body = std::fs::read_to_string("tests/fixtures/files.json").unwrap();
        let expired = server.mock(|when, then| {
            when.method(GET).path("/library/get-file-list").query_param("folderID", "9");
            then.status(302).header("location", "/login");
        });
        // Delete-and-replace pattern isn't available; instead assert login was attempted.
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let _ = expired; let _ = body;
        // With only the expired mock present, a re-auth attempt still ends in Auth error,
        // but login() must have been called exactly once (retry, not infinite loop).
        let res = client.list_folder(9);
        assert!(matches!(res, Err(ClientError::Auth)));
    }
```

Rationale: httpmock cannot easily change a response between calls, so this test verifies the retry path terminates (one re-auth, then gives up) rather than looping forever. The happy re-auth path is exercised by the live test in Task 12.

- [ ] **Step 2: Run test to verify it fails or hangs**

Run: `cargo test --lib client::tests::list_folder_reauths -- --nocapture`
Expected: Currently `list_folder` returns `Auth` immediately (no login attempt) — the test passes trivially now, so first make the assertion stronger by counting login hits:

Replace the test body's final lines with a login-hit counter:

```rust
        let login_hits = server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let res = client.list_folder(9);
        assert!(matches!(res, Err(ClientError::Auth)));
        assert_eq!(login_hits.hits(), 1, "should re-auth exactly once then give up");
```

Run again; expected FAIL: `login_hits` is 0 because re-auth isn't wired yet.

- [ ] **Step 3: Implement `with_reauth` and wrap the three methods**

Add the helper in `impl HttpCondoClient`:

```rust
    fn with_reauth<T>(&self, op: impl Fn() -> Result<T, ClientError>) -> Result<T, ClientError> {
        match op() {
            Err(ClientError::Auth) => {
                log::info!("session expired; re-authenticating");
                self.login()?;
                op()
            }
            other => other,
        }
    }
```

Refactor `list_folder`, `file_meta`, `download_file` so their HTTP body lives in a closure passed to `with_reauth`. Example for `list_folder`:

```rust
    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
        self.with_reauth(|| self.list_folder_once(folder_id))
    }
```

Rename the previous implementations to `list_folder_once`, `file_meta_once`, `download_file_once` as **inherent** methods on `HttpCondoClient` (private), and have the trait methods delegate through `with_reauth`. `download_file` takes `&mut dyn Write`, which is not `Fn`-friendly; for it, wrap manually:

```rust
    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError> {
        match self.download_file_once(file_id, out) {
            Err(ClientError::Auth) => { self.login()?; self.download_file_once(file_id, out) }
            other => other,
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib client`
Expected: PASS (all client tests, including the re-auth counter = 1).

- [ ] **Step 5: Commit**

```bash
git add src/client.rs
git commit -m "add transparent re-authentication with single retry"
```

---

## Task 9: Caches — content (disk) + metadata TTL

**Files:**
- Modify: `src/cache.rs`

**Interfaces:**
- Produces:
  - `pub struct ContentCache { root: PathBuf }` with `pub fn new(root: PathBuf) -> std::io::Result<ContentCache>`, `pub fn path_for(&self, file_id: u64, date: &str) -> PathBuf`, `pub fn get(&self, file_id: u64, date: &str) -> Option<PathBuf>` (returns the path iff it already exists), and `pub fn store_from<F>(&self, file_id: u64, date: &str, fill: F) -> std::io::Result<PathBuf> where F: FnOnce(&mut std::fs::File) -> std::io::Result<()>` (writes to a temp file then atomically renames, so a partial download never looks complete).
  - `pub struct TtlCache<K, V> { ttl: Duration, map: Mutex<HashMap<K, (Instant, V)>> }` with `new(ttl)`, `get(&self, k) -> Option<V>` (clones V; returns None if expired/missing), `put(&self, k, v)`, and `invalidate(&self, k)`.

- [ ] **Step 1: Write failing tests in `src/cache.rs`**

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_cache_stores_and_hits_by_id_and_date() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf()).unwrap();
        assert!(cache.get(1, "2025-01-01 00:00:00").is_none());
        let p = cache
            .store_from(1, "2025-01-01 00:00:00", |f| {
                use std::io::Write;
                f.write_all(b"data")
            })
            .unwrap();
        assert!(p.exists());
        assert_eq!(std::fs::read(&p).unwrap(), b"data");
        // same id+date -> hit
        assert!(cache.get(1, "2025-01-01 00:00:00").is_some());
        // changed date -> miss (file was re-uploaded)
        assert!(cache.get(1, "2025-02-02 00:00:00").is_none());
    }

    #[test]
    fn ttl_cache_expires() {
        let c: TtlCache<u64, String> = TtlCache::new(Duration::from_millis(30));
        c.put(7, "hi".into());
        assert_eq!(c.get(&7), Some("hi".to_string()));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.get(&7), None);
    }

    #[test]
    fn ttl_cache_invalidate() {
        let c: TtlCache<u64, String> = TtlCache::new(Duration::from_secs(60));
        c.put(1, "a".into());
        c.invalidate(&1);
        assert_eq!(c.get(&1), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib cache`
Expected: FAIL — types not found.

- [ ] **Step 3: Implement** (add above `#[cfg(test)]`)

```rust
pub struct ContentCache {
    root: PathBuf,
}

impl ContentCache {
    pub fn new(root: PathBuf) -> std::io::Result<ContentCache> {
        std::fs::create_dir_all(&root)?;
        Ok(ContentCache { root })
    }

    fn key(file_id: u64, date: &str) -> String {
        // date may contain spaces/colons; hash-free but filesystem-safe
        let safe: String = date.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        format!("{file_id}-{safe}.bin")
    }

    pub fn path_for(&self, file_id: u64, date: &str) -> PathBuf {
        self.root.join(Self::key(file_id, date))
    }

    pub fn get(&self, file_id: u64, date: &str) -> Option<PathBuf> {
        let p = self.path_for(file_id, date);
        if p.exists() { Some(p) } else { None }
    }

    pub fn store_from<F>(&self, file_id: u64, date: &str, fill: F) -> std::io::Result<PathBuf>
    where
        F: FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    {
        let final_path = self.path_for(file_id, date);
        let tmp = self.root.join(format!(".tmp-{file_id}-{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            fill(&mut f)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &final_path)?;
        Ok(final_path)
    }
}

pub struct TtlCache<K, V> {
    ttl: Duration,
    map: Mutex<HashMap<K, (Instant, V)>>,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> TtlCache<K, V> {
    pub fn new(ttl: Duration) -> TtlCache<K, V> {
        TtlCache { ttl, map: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, k: &K) -> Option<V> {
        let mut map = self.map.lock().unwrap();
        if let Some((at, v)) = map.get(k) {
            if at.elapsed() <= self.ttl {
                return Some(v.clone());
            }
            map.remove(k);
        }
        None
    }

    pub fn put(&self, k: K, v: V) {
        self.map.lock().unwrap().insert(k, (Instant::now(), v));
    }

    pub fn invalidate(&self, k: &K) {
        self.map.lock().unwrap().remove(k);
    }
}
```

Note: `TtlCache::get` uses `Instant::now()`/`elapsed()` — fine in real code and tests. Keep `path_for`/`key`/`Path` referenced to avoid dead-code warnings (they are used by later tasks).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib cache`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/cache.rs
git commit -m "add on-disk content cache and in-memory TTL cache"
```

---

## Task 10: Vfs — inode table and read-only operations

**Files:**
- Modify: `src/vfs.rs`

**Interfaces:**
- Consumes: `CondoClient`, `Entry`, caches, `names::*`.
- Produces:
  - `pub enum VfsError { NotFound, Io }` with `pub fn errno(&self) -> i32` (`NotFound`→`libc::ENOENT`, `Io`→`libc::EIO`).
  - `pub struct DirEntry { pub ino: u64, pub name: String, pub kind: fuser::FileType }`.
  - `pub struct Attr { pub ino: u64, pub size: u64, pub kind: fuser::FileType, pub mtime: std::time::SystemTime }` (a thin, testable subset; the fuser adapter expands it to `fuser::FileAttr`).
  - `pub struct Vfs<C: CondoClient> { … }` with:
    - `pub fn new(client: C, root_folder_id: u64, cache: ContentCache, meta_ttl: Duration, uid: u32, gid: u32) -> Vfs<C>`
    - `pub fn readdir(&self, ino: u64) -> Result<Vec<DirEntry>, VfsError>`
    - `pub fn lookup(&self, parent: u64, name: &str) -> Result<Attr, VfsError>`
    - `pub fn getattr(&self, ino: u64) -> Result<Attr, VfsError>`
    - `pub fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, VfsError>`
  - Root inode is `fuser::FUSE_ROOT_ID` (=1) and maps to `root_folder_id`.

Design notes for the implementer:
- Keep all mutable state behind a single `Mutex<Inner>`.
- `Inner` holds: `next_ino: u64` (start at 2), `nodes: HashMap<u64, Node>`, `id_to_ino: HashMap<NodeKey, u64>` (stable inode reuse across re-listings), `children: HashMap<u64, Vec<u64>>` (dir listing), `child_by_name: HashMap<(u64, String), u64>`, `listed_at: HashMap<u64, Instant>`.
- `Node` enum: `Folder { id: u64 }`, `File { id: u64, name: String, date: String, thumbnail: String, size: Option<u64> }`.
- `NodeKey`: `Folder(u64)` or `File(u64)` — used to reuse the same inode when the same Condo id reappears.
- On `new`, seed inode 1 → `Node::Folder { id: root_folder_id }`.
- `ensure_listed(ino)`: if `listed_at` is missing or older than `meta_ttl`, call `client.list_folder(folder_id)`, build display names (`names::file_display_name` for files, `names::sanitize_name` for folders), run `names::resolve_collisions` over the ordered names, allocate/reuse child inodes, and refresh `children`/`child_by_name`/`listed_at`.
- `getattr` on a file with `size == None`: call `client.file_meta(id)`, store the size on the node.
- `read`: ensure the file is in `ContentCache` (download once via `client.download_file` into `store_from`), then read the requested slice from the cached file. Clamp `offset`/`size` to file length.

- [ ] **Step 1: Write a mock client + failing tests in `src/vfs.rs`**

```rust
use crate::cache::ContentCache;
use crate::client::{ClientError, CondoClient};
use crate::model::{Entry, FileMeta};
use crate::names;
use fuser::FileType;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct MockClient {
        folders: HashMap<u64, Vec<Entry>>,
        files: HashMap<u64, (Vec<u8>, u64)>, // id -> (bytes, size)
        meta_calls: AtomicU32,
        download_calls: AtomicU32,
    }

    impl CondoClient for MockClient {
        fn login(&self) -> Result<(), ClientError> { Ok(()) }
        fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
            Ok(self.folders.get(&folder_id).cloned().unwrap_or_default())
        }
        fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError> {
            self.meta_calls.fetch_add(1, Ordering::SeqCst);
            let (_, size) = self.files.get(&file_id).ok_or(ClientError::NotFound)?;
            Ok(FileMeta { size: *size, filename: None })
        }
        fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            let (bytes, _) = self.files.get(&file_id).ok_or(ClientError::NotFound)?;
            out.write_all(bytes)?;
            Ok(bytes.len() as u64)
        }
    }

    fn fixture_vfs() -> (Vfs<MockClient>, tempfile::TempDir) {
        // root folder 100 contains subfolder 200 ("Reports") and file 300 ("01/09/25 Notes", pdf)
        let mut folders = HashMap::new();
        folders.insert(100, vec![
            Entry::Folder { id: 200, name: "Reports".into() },
            Entry::File { id: 300, key: "K".into(), name: "01/09/25 Notes".into(),
                          date: "2025-01-18 02:41:25".into(),
                          thumbnail: "/shared/images/icons/pdf-128x128.png".into() },
        ]);
        folders.insert(200, vec![]);
        let mut files = HashMap::new();
        files.insert(300u64, (b"%PDF-1.7 body".to_vec(), 13u64));
        let client = MockClient { folders, files, meta_calls: AtomicU32::new(0), download_calls: AtomicU32::new(0) };
        let dir = tempfile::tempdir().unwrap();
        let cache = ContentCache::new(dir.path().to_path_buf()).unwrap();
        (Vfs::new(client, 100, cache, Duration::from_secs(60), 1000, 1000), dir)
    }

    #[test]
    fn readdir_root_lists_folder_and_file_with_clean_names() {
        let (vfs, _d) = fixture_vfs();
        let mut entries = vfs.readdir(fuser::FUSE_ROOT_ID).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["01-09-25 Notes.pdf".to_string(), "Reports".to_string()]);
        let file = entries.iter().find(|e| e.name.ends_with(".pdf")).unwrap();
        assert_eq!(file.kind, FileType::RegularFile);
    }

    #[test]
    fn lookup_then_read_returns_file_bytes() {
        let (vfs, _d) = fixture_vfs();
        let attr = vfs.lookup(fuser::FUSE_ROOT_ID, "01-09-25 Notes.pdf").unwrap();
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.size, 13);
        let data = vfs.read(attr.ino, 0, 100).unwrap();
        assert_eq!(data, b"%PDF-1.7 body");
        // offset read
        let tail = vfs.read(attr.ino, 9, 100).unwrap();
        assert_eq!(tail, b"body");
    }

    #[test]
    fn lookup_missing_is_enoent() {
        let (vfs, _d) = fixture_vfs();
        let err = vfs.lookup(fuser::FUSE_ROOT_ID, "nope").unwrap_err();
        assert_eq!(err.errno(), libc::ENOENT);
    }

    #[test]
    fn inodes_are_stable_across_relisting() {
        let (vfs, _d) = fixture_vfs();
        let a = vfs.lookup(fuser::FUSE_ROOT_ID, "Reports").unwrap().ino;
        let _ = vfs.readdir(fuser::FUSE_ROOT_ID).unwrap();
        let b = vfs.lookup(fuser::FUSE_ROOT_ID, "Reports").unwrap().ino;
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib vfs`
Expected: FAIL — `Vfs` etc. not found.

- [ ] **Step 3: Implement `Vfs`** (add above `#[cfg(test)]`). Full implementation:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError { NotFound, Io }

impl VfsError {
    pub fn errno(&self) -> i32 {
        match self {
            VfsError::NotFound => libc::ENOENT,
            VfsError::Io => libc::EIO,
        }
    }
}

impl From<ClientError> for VfsError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::NotFound => VfsError::NotFound,
            _ => VfsError::Io,
        }
    }
}
impl From<std::io::Error> for VfsError {
    fn from(_: std::io::Error) -> Self { VfsError::Io }
}

pub struct DirEntry { pub ino: u64, pub name: String, pub kind: FileType }
pub struct Attr { pub ino: u64, pub size: u64, pub kind: FileType, pub mtime: SystemTime }

#[derive(Clone)]
enum Node {
    Folder { id: u64 },
    File { id: u64, date: String, thumbnail: String, size: Option<u64> },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NodeKey { Folder(u64), File(u64) }

struct Inner {
    next_ino: u64,
    nodes: HashMap<u64, Node>,
    id_to_ino: HashMap<NodeKey, u64>,
    children: HashMap<u64, Vec<u64>>,
    child_by_name: HashMap<(u64, String), u64>,
    listed_at: HashMap<u64, Instant>,
}

pub struct Vfs<C: CondoClient> {
    client: C,
    cache: ContentCache,
    meta_ttl: Duration,
    uid: u32,
    gid: u32,
    inner: Mutex<Inner>,
}

impl<C: CondoClient> Vfs<C> {
    pub fn new(client: C, root_folder_id: u64, cache: ContentCache, meta_ttl: Duration, uid: u32, gid: u32) -> Vfs<C> {
        let mut nodes = HashMap::new();
        nodes.insert(fuser::FUSE_ROOT_ID, Node::Folder { id: root_folder_id });
        let mut id_to_ino = HashMap::new();
        id_to_ino.insert(NodeKey::Folder(root_folder_id), fuser::FUSE_ROOT_ID);
        Vfs {
            client, cache, meta_ttl, uid, gid,
            inner: Mutex::new(Inner {
                next_ino: 2,
                nodes,
                id_to_ino,
                children: HashMap::new(),
                child_by_name: HashMap::new(),
                listed_at: HashMap::new(),
            }),
        }
    }

    pub fn uid(&self) -> u32 { self.uid }
    pub fn gid(&self) -> u32 { self.gid }

    fn attr_for(&self, ino: u64, node: &Node) -> Attr {
        match node {
            Node::Folder { .. } => Attr { ino, size: 0, kind: FileType::Directory, mtime: SystemTime::UNIX_EPOCH },
            Node::File { date, size, .. } => Attr {
                ino,
                size: size.unwrap_or(0),
                kind: FileType::RegularFile,
                mtime: names::parse_condo_date(date),
            },
        }
    }

    fn ensure_listed(&self, ino: u64) -> Result<(), VfsError> {
        let folder_id = {
            let inner = self.inner.lock().unwrap();
            match inner.nodes.get(&ino) {
                Some(Node::Folder { id }) => *id,
                Some(Node::File { .. }) => return Err(VfsError::Io), // not a dir
                None => return Err(VfsError::NotFound),
            }
        };
        {
            let inner = self.inner.lock().unwrap();
            if let Some(at) = inner.listed_at.get(&ino) {
                if at.elapsed() <= self.meta_ttl { return Ok(()); }
            }
        }
        let entries = self.client.list_folder(folder_id)?;

        // Build display names in listing order, then resolve collisions.
        let display: Vec<String> = entries.iter().map(|e| match e {
            Entry::Folder { name, .. } => names::sanitize_name(name),
            Entry::File { name, thumbnail, .. } => names::file_display_name(name, thumbnail),
        }).collect();
        let display = names::resolve_collisions(display);

        let mut inner = self.inner.lock().unwrap();
        let mut kids = Vec::with_capacity(entries.len());
        // clear stale name index for this dir
        inner.child_by_name.retain(|(p, _), _| *p != ino);
        for (entry, name) in entries.into_iter().zip(display.into_iter()) {
            let (key, node) = match entry {
                Entry::Folder { id, .. } => (NodeKey::Folder(id), Node::Folder { id }),
                Entry::File { id, date, thumbnail, .. } => {
                    // preserve a previously-learned size if the same file id is already known
                    let prev_size = inner.id_to_ino.get(&NodeKey::File(id))
                        .and_then(|ci| inner.nodes.get(ci))
                        .and_then(|n| if let Node::File { size, .. } = n { *size } else { None });
                    (NodeKey::File(id), Node::File { id, date, thumbnail, size: prev_size })
                }
            };
            let child_ino = match inner.id_to_ino.get(&key) {
                Some(ci) => *ci,
                None => {
                    let ci = inner.next_ino;
                    inner.next_ino += 1;
                    inner.id_to_ino.insert(key, ci);
                    ci
                }
            };
            inner.nodes.insert(child_ino, node);
            inner.child_by_name.insert((ino, name.clone()), child_ino);
            kids.push(child_ino);
        }
        inner.children.insert(ino, kids);
        inner.listed_at.insert(ino, Instant::now());
        Ok(())
    }

    pub fn readdir(&self, ino: u64) -> Result<Vec<DirEntry>, VfsError> {
        self.ensure_listed(ino)?;
        let inner = self.inner.lock().unwrap();
        let kids = inner.children.get(&ino).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for (_, ci) in inner.child_by_name.iter().filter(|((p, _), _)| *p == ino).map(|((_, n), ci)| (n.clone(), *ci)).collect::<Vec<_>>() {
            let _ = ci; // names come from child_by_name below
        }
        // Reconstruct (name, ino) pairs for this dir from child_by_name.
        let mut pairs: Vec<(String, u64)> = inner.child_by_name.iter()
            .filter(|((p, _), _)| *p == ino)
            .map(|((_, n), ci)| (n.clone(), *ci))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, ci) in pairs {
            if !kids.contains(&ci) { continue; }
            let kind = match inner.nodes.get(&ci) {
                Some(Node::Folder { .. }) => FileType::Directory,
                _ => FileType::RegularFile,
            };
            out.push(DirEntry { ino: ci, name, kind });
        }
        Ok(out)
    }

    pub fn lookup(&self, parent: u64, name: &str) -> Result<Attr, VfsError> {
        self.ensure_listed(parent)?;
        let (ino, node) = {
            let inner = self.inner.lock().unwrap();
            let ino = *inner.child_by_name.get(&(parent, name.to_string())).ok_or(VfsError::NotFound)?;
            let node = inner.nodes.get(&ino).ok_or(VfsError::NotFound)?.clone();
            (ino, node)
        };
        // For files without a known size, fetch it now (getattr/lookup needs a real size).
        self.ensure_size(ino, &node)
    }

    pub fn getattr(&self, ino: u64) -> Result<Attr, VfsError> {
        let node = {
            let inner = self.inner.lock().unwrap();
            inner.nodes.get(&ino).ok_or(VfsError::NotFound)?.clone()
        };
        self.ensure_size(ino, &node)
    }

    fn ensure_size(&self, ino: u64, node: &Node) -> Result<Attr, VfsError> {
        if let Node::File { id, size: None, .. } = node {
            let meta = self.client.file_meta(*id)?;
            let mut inner = self.inner.lock().unwrap();
            if let Some(Node::File { size, .. }) = inner.nodes.get_mut(&ino) {
                *size = Some(meta.size);
            }
            let updated = inner.nodes.get(&ino).cloned().ok_or(VfsError::NotFound)?;
            return Ok(self.attr_for(ino, &updated));
        }
        Ok(self.attr_for(ino, node))
    }

    pub fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, VfsError> {
        let (id, date) = {
            let inner = self.inner.lock().unwrap();
            match inner.nodes.get(&ino) {
                Some(Node::File { id, date, .. }) => (*id, date.clone()),
                Some(Node::Folder { .. }) => return Err(VfsError::Io),
                None => return Err(VfsError::NotFound),
            }
        };
        // Ensure the file is cached on disk (download whole file once; no server-side ranges).
        let path = match self.cache.get(id, &date) {
            Some(p) => p,
            None => self.cache.store_from(id, &date, |f| {
                self.client.download_file(id, f).map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "download failed"))?;
                Ok(())
            })?,
        };
        let mut file = std::fs::File::open(&path)?;
        let len = file.metadata()?.len();
        if offset >= len { return Ok(Vec::new()); }
        file.seek(SeekFrom::Start(offset))?;
        let want = std::cmp::min(size as u64, len - offset) as usize;
        let mut buf = vec![0u8; want];
        let n = read_full(&mut file, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}
```

Note: the first `for` loop in `readdir` is dead scaffolding — delete it; the `pairs` block is the real one. (Left here only to flag that `child_by_name` is the source of names; remove the stray loop when implementing.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vfs`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/vfs.rs
git commit -m "add Vfs inode table and read-only operations"
```

---

## Task 11: fuser adapter + config + CLI + main

**Files:**
- Modify: `src/fs.rs`, `src/config.rs`, `src/bin/condo-fuse.rs`

**Interfaces:**
- Consumes: everything above. Produces:
  - `src/fs.rs`: `pub struct CondoFs<C: CondoClient> { vfs: Vfs<C> }` implementing `fuser::Filesystem` (`lookup`, `getattr`, `readdir`, `open`, `read`). Plus `fn to_file_attr(a: &Attr, uid: u32, gid: u32) -> fuser::FileAttr`.
  - `src/config.rs`: `pub struct Config { pub credentials: PathBuf, pub root: u64, pub mountpoint: PathBuf, pub cache_dir: PathBuf, pub meta_ttl: Duration, pub foreground: bool }` and a `clap`-derived `Cli`.
  - `src/bin/condo-fuse.rs`: parses args, builds client, logs in, mounts.

This task is validated by a real mount rather than unit tests (the pieces underneath are already unit-tested).

- [ ] **Step 1: Implement `src/fs.rs`**

```rust
use crate::client::CondoClient;
use crate::vfs::{Attr, Vfs, VfsError};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use std::ffi::OsStr;
use std::time::Duration;

const TTL: Duration = Duration::from_secs(1);

pub struct CondoFs<C: CondoClient> {
    vfs: Vfs<C>,
}

impl<C: CondoClient> CondoFs<C> {
    pub fn new(vfs: Vfs<C>) -> CondoFs<C> {
        CondoFs { vfs }
    }
}

fn to_file_attr(a: &Attr, uid: u32, gid: u32) -> FileAttr {
    let (perm, nlink) = match a.kind {
        FileType::Directory => (0o555, 2),
        _ => (0o444, 1),
    };
    let blocks = (a.size + 511) / 512;
    FileAttr {
        ino: a.ino,
        size: a.size,
        blocks,
        atime: a.mtime,
        mtime: a.mtime,
        ctime: a.mtime,
        crtime: a.mtime,
        kind: a.kind,
        perm,
        nlink,
        uid,
        gid,
        rdev: 0,
        blksize: 512,
        flags: 0,
    }
}

impl<C: CondoClient> Filesystem for CondoFs<C> {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(libc::ENOENT),
        };
        match self.vfs.lookup(parent, name) {
            Ok(attr) => reply.entry(&TTL, &to_file_attr(&attr, self.vfs.uid(), self.vfs.gid()), 0),
            Err(e) => reply.error(e.errno()),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match self.vfs.getattr(ino) {
            Ok(attr) => reply.attr(&TTL, &to_file_attr(&attr, self.vfs.uid(), self.vfs.gid())),
            Err(e) => reply.error(e.errno()),
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let offset = if offset < 0 { 0 } else { offset as u64 };
        match self.vfs.read(ino, offset, size) {
            Ok(data) => reply.data(&data),
            Err(e) => reply.error(e.errno()),
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        let entries = match self.vfs.readdir(ino) {
            Ok(e) => e,
            Err(e) => return reply.error(e.errno()),
        };
        // "." and ".." first
        let mut listing: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".to_string()),
            (ino, FileType::Directory, "..".to_string()),
        ];
        for e in entries {
            listing.push((e.ino, e.kind, e.name));
        }
        for (i, (child_ino, kind, name)) in listing.into_iter().enumerate().skip(offset as usize) {
            // add returns true when the buffer is full
            if reply.add(child_ino, (i + 1) as i64, kind, &name) {
                break;
            }
        }
        reply.ok();
    }
}

// Silence unused import in some build configs.
#[allow(unused_imports)]
use VfsError as _VfsError;
```

Note: the `getattr` signature (with `_fh: Option<u64>`) matches `fuser` 0.15. If `cargo build` reports a signature mismatch, run `cargo doc -p fuser --open` or check the compiler's expected signature and adjust the method to match exactly.

- [ ] **Step 2: Implement `src/config.rs`**

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "condo-fuse", about = "Mount a Condo Control File Library as a read-only filesystem")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Mount the library at MOUNTPOINT
    Mount(MountArgs),
}

#[derive(Parser)]
pub struct MountArgs {
    /// Path to the mountpoint (must be an existing empty directory)
    pub mountpoint: PathBuf,

    /// Credentials file (KEY=VALUE lines: USERNAME, PASSWORD)
    #[arg(long, default_value = "~/tokens/condo-control.txt")]
    pub credentials: String,

    /// Root folder ID in the library
    #[arg(long, default_value_t = 137473)]
    pub root: u64,

    /// Directory for the on-disk content cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Seconds to cache directory listings before refetching
    #[arg(long, default_value_t = 60)]
    pub meta_ttl: u64,

    /// Stay in the foreground (do not daemonize); logs to stderr
    #[arg(long, default_value_t = true)]
    pub foreground: bool,
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

impl MountArgs {
    pub fn credentials_path(&self) -> PathBuf { expand_tilde(&self.credentials) }
    pub fn cache_dir_path(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| {
            dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join("condo-fuse")
        })
    }
    pub fn meta_ttl_dur(&self) -> Duration { Duration::from_secs(self.meta_ttl) }
}
```

- [ ] **Step 3: Implement `src/bin/condo-fuse.rs`**

```rust
use clap::Parser;
use condo_fuse::cache::ContentCache;
use condo_fuse::client::{CondoClient, HttpCondoClient};
use condo_fuse::config::{Cli, Command};
use condo_fuse::credentials::Credentials;
use condo_fuse::fs::CondoFs;
use condo_fuse::vfs::Vfs;
use fuser::MountOption;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    match cli.command {
        Command::Mount(args) => {
            if let Err(e) = run_mount(args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn run_mount(args: condo_fuse::config::MountArgs) -> Result<(), Box<dyn std::error::Error>> {
    let creds = Credentials::from_file(&args.credentials_path())?;
    let client = HttpCondoClient::new("https://app.condocontrol.com", creds)?;
    log::info!("authenticating…");
    client.login()?;
    log::info!("authenticated; mounting {} at {}", args.root, args.mountpoint.display());

    let cache = ContentCache::new(args.cache_dir_path())?;
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let vfs = Vfs::new(client, args.root, cache, args.meta_ttl_dur(), uid, gid);
    let fs = CondoFs::new(vfs);

    let options = vec![
        MountOption::RO,
        MountOption::FSName("condo".to_string()),
        MountOption::Subtype("condofuse".to_string()),
        MountOption::DefaultPermissions,
    ];
    fuser::mount2(fs, &args.mountpoint, &options)?;
    Ok(())
}
```

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles. Fix any `fuser` trait-method signature mismatches by matching the compiler's expected signatures exactly (fuser 0.15).

- [ ] **Step 5: Smoke-test the CLI surface (no network)**

Run: `cargo run --bin condo-fuse -- --help` and `cargo run --bin condo-fuse -- mount --help`
Expected: usage text listing the `mount` subcommand and flags (`--credentials`, `--root`, `--cache-dir`, `--meta-ttl`).

- [ ] **Step 6: Commit**

```bash
git add src/fs.rs src/config.rs src/bin/condo-fuse.rs
git commit -m "add fuser adapter, config/CLI, and mount entrypoint"
```

---

## Task 12: Live end-to-end validation

**Files:**
- Create: `tests/live_smoke.rs` (env-gated integration test)

**Interfaces:**
- Consumes the public crate API. Runs only when `CONDO_LIVE=1` and a real credentials file is present.

- [ ] **Step 1: Write `tests/live_smoke.rs`**

```rust
// Opt-in live test. Run with:
//   CONDO_LIVE=1 CONDO_CREDS=~/tokens/condo-control.txt cargo test --test live_smoke -- --nocapture
use condo_fuse::client::{CondoClient, HttpCondoClient};
use condo_fuse::credentials::Credentials;
use condo_fuse::model::Entry;
use std::path::PathBuf;

fn creds_path() -> PathBuf {
    let p = std::env::var("CONDO_CREDS").unwrap_or_else(|_| "~/tokens/condo-control.txt".into());
    if let Some(rest) = p.strip_prefix("~/") {
        return dirs::home_dir().unwrap().join(rest);
    }
    PathBuf::from(p)
}

#[test]
fn live_login_and_list_root() {
    if std::env::var("CONDO_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live test (set CONDO_LIVE=1 to run)");
        return;
    }
    let creds = Credentials::from_file(&creds_path()).unwrap();
    let client = HttpCondoClient::new("https://app.condocontrol.com", creds).unwrap();
    client.login().expect("login should succeed");
    let entries = client.list_folder(137473).expect("root listing should succeed");
    assert!(!entries.is_empty(), "root folder should contain entries");
    let has_folder = entries.iter().any(|e| matches!(e, Entry::Folder { .. }));
    assert!(has_folder, "root should contain at least one folder");
    eprintln!("root has {} entries", entries.len());
}
```

- [ ] **Step 2: Confirm it is skipped by default**

Run: `cargo test --test live_smoke`
Expected: PASS (prints "skipping live test").

- [ ] **Step 3: Run it live**

Run: `CONDO_LIVE=1 cargo test --test live_smoke -- --nocapture`
Expected: PASS; prints a non-zero root entry count.

- [ ] **Step 4: Real mount validation (manual)**

```bash
mkdir -p /tmp/condo-mnt
cargo run --bin condo-fuse -- mount --credentials ~/tokens/condo-control.txt --root 137473 /tmp/condo-mnt &
sleep 3
ls -la /tmp/condo-mnt
# descend into a known folder and read a file end-to-end
ls -la "/tmp/condo-mnt/Board of Directors"
# pick a PDF and verify it is a valid PDF (starts with %PDF and has non-zero size)
# then unmount:
fusermount3 -u /tmp/condo-mnt
```

Expected: root lists real folder names; descending lists files with `.pdf` extensions and non-zero sizes; copying a PDF out yields a file whose first bytes are `%PDF`.

- [ ] **Step 5: Commit**

```bash
git add tests/live_smoke.rs
git commit -m "add opt-in live smoke test"
```

- [ ] **Step 6: Final full test run**

Run: `cargo test`
Expected: all unit tests pass; live test reports skipped.

---

## Self-Review Notes (already reconciled)

- **Spec coverage:** login (T5), get-file-list listing (T6), download + size-without-body (T7), re-auth (T8), name sanitization/extension/collision (T4), caching + TTL (T9), inode/tree + read-only FUSE ops (T10/T11), config/CLI defaults (T11), fixtures + mock-client tests + live test (T2/T3/T10/T12). `get-folder-hierarchy` was intentionally dropped (YAGNI — per-folder `get-file-list` suffices; spec marked it optional).
- **Read-only:** enforced via `MountOption::RO` and the absence of any write path; no `write`/`create`/`unlink`/`rename` implemented (fuser defaults them to `ENOSYS`).
- **Type consistency:** `CondoClient` signatures identical across client, mock, and vfs; `Attr`/`DirEntry` produced by `Vfs` and consumed by `CondoFs`; `VfsError::errno()` used by the adapter.
- **Known implementer watch-points (call out, don't silently fix):** exact `fuser` 0.15 trait-method signatures (adjust to compiler); the `file_meta` header-only assumption (validated live in T12 — if the server streams the whole body anyway, it still works, just less efficiently); remove the stray dead loop noted in T10 `readdir`.
