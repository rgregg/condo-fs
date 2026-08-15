# condo-mcp Announcements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `list_announcements` and `get_announcement` MCP tools over Condo Control's announcements module.

**Architecture:** Same three-layer shape as the library and calendar domains — `model/announcements.rs` parses, `client/announcements.rs` fetches, `tools/announcements.rs` exposes. The difference: these responses are **server-rendered HTML**, so the model layer parses a DOM instead of deserializing JSON.

**Tech Stack:** Rust 2021 (1.95), blocking `reqwest`, `rmcp` 3, `scraper` for HTML parsing, `htmd` for HTML→Markdown, `chrono` for dates, `httpmock` for tests.

**Source spec:** `docs/superpowers/specs/2026-08-10-condo-mcp-design.md` (Announcements section, written after discovery)

## Discovery findings this plan is built on

Established by probing the live site on 2026-08-14:

- **List:** `GET /announcement/show-list` — a full HTML page. The content is a
  `table.table-striped`; each row has
  `a.announcementTitle[href="/announcement/detail/{id}"]` with the title as its text, then a
  `<span>` holding the posted timestamp, then optionally `<span>Expires: …</span>`.
- **Detail:** `GET /announcement/detail/{id}` — a full HTML page with a labeled metadata
  block: `label.control-label` reading `Title:`, `Expires:`, `Posted:`, `Posted By:`, each
  followed by a `p.form-control-static` holding the value. The body is CKEditor HTML inside
  `div.ck-content`. Attachments follow in a `table.table-striped` with File Name and Date
  Uploaded columns.
- **Dates** are US-format local strings: `8/13/2026 4:14:37 PM`. No timezone.
- **Pagination is a dead end.** The pager renders with every control disabled and the
  date-range filter is a **POST** form. A GET-only client sees exactly one page of
  currently-posted, non-expired announcements (14 at discovery). Do not try to page or filter.
- Trimmed captures are committed at `condo-api/tests/fixtures/announcements_list.html` and
  `condo-api/tests/fixtures/announcement_detail.html`.

## Global Constraints

- **Read-only.** GET only. The list page's filter form is a POST — do not submit it.
  `scripts/guardrails.sh` enforces this in CI and must keep passing.
- Rust edition 2021, `rust-version = "1.95"`, license MIT.
- `condo-api` stays **blocking**; `condo-mcp` bridges with `spawn_blocking`.
- The password is never logged, never in an error message, never serialized.
- In `condo-mcp`, stdout is the MCP wire protocol; logging goes to stderr.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all --all-features`, and `./scripts/guardrails.sh` must pass.
- Tool descriptions are read by agents deciding whether to call them; they must state what is
  returned **and its limits** — here, that the archive is unreachable.

## A note on parsing HTML

Anchor every selector on class names (`.announcementTitle`, `.form-control-static`,
`.ck-content`) and on the label text, never on document position, sibling index, or element
order. The surrounding page is a full application shell — nav, analytics, feature widgets —
that changes independently of the content, and the fixtures are trimmed excerpts of it.

A parse that finds nothing must be distinguishable from a page that genuinely has no
announcements. An empty list is a legitimate result; a page whose table is missing entirely
means the markup moved and must surface as an error, not as "no announcements."

---

### Task 1: `list_announcements`

**Files:**
- Create: `condo-api/src/model/announcements.rs`, `condo-api/src/client/announcements.rs`,
  `condo-mcp/src/tools/announcements.rs`
- Modify: `Cargo.toml` (root), `condo-api/Cargo.toml`, `condo-api/src/model/mod.rs`,
  `condo-api/src/client/mod.rs`, `condo-api/src/lib.rs`, `condo-fs/src/vfs.rs`,
  `condo-mcp/src/tools/mod.rs`, `condo-mcp/src/server.rs`
- Test: unit tests in the two `condo-api` files; integration tests in `condo-mcp/tests/server.rs`

**Interfaces:**
- Consumes: `HttpCondoClient`'s `pub(crate)` `http`, `url`, `with_reauth`; `ClientError`;
  `CondoMcp::with_client`, `CondoMcp::tool_error`, `CondoMcp::build_router`.
- Produces:
  - `condo_api::AnnouncementSummary { id: u64, title: String, posted_at: NaiveDateTime, expires_at: Option<NaiveDateTime> }`
  - `condo_api::parse_announcement_list(html: &str) -> Result<Vec<AnnouncementSummary>, ParseError>`
  - `condo_api::ParseError` — a new error type for HTML parsing failures (see Step 3)
  - `CondoClient::list_announcements(&self) -> Result<Vec<AnnouncementSummary>, ClientError>`
  - MCP tool `list_announcements`, no parameters

- [ ] **Step 1: Add the HTML dependencies**

Root `Cargo.toml` `[workspace.dependencies]`:

```toml
scraper = "0.20"
htmd = "0.5"
```

Add `scraper = { workspace = true }` and `htmd = { workspace = true }` to
`condo-api/Cargo.toml` `[dependencies]`. Both belong to `condo-api` — parsing is a model-layer
concern and `condo-mcp` must not need to know the responses were ever HTML.

- [ ] **Step 2: Write the failing list-parser tests**

Create `condo-api/src/model/announcements.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string("tests/fixtures/announcements_list.html").unwrap()
    }

    #[test]
    fn parses_every_row_of_the_captured_list() {
        let items = parse_announcement_list(&fixture()).unwrap();
        assert_eq!(items.len(), 3);
        let first = &items[0];
        assert_eq!(first.id, 1541631);
        assert_eq!(first.title, "Walk-About Garage Sale - Labor Day Weekend!");
        assert_eq!(
            first.posted_at,
            chrono::NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(16, 14, 37)
                .unwrap(),
            "4:14:37 PM must parse as 16:14:37, not 04:14:37"
        );
        assert_eq!(
            first.expires_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap()
        );
    }

    #[test]
    fn newest_first() {
        let items = parse_announcement_list(&fixture()).unwrap();
        let mut sorted = items.clone();
        sorted.sort_by(|a, b| b.posted_at.cmp(&a.posted_at));
        assert_eq!(items, sorted, "callers asking 'what's new' depend on this order");
    }

    #[test]
    fn an_empty_table_is_an_empty_list_not_an_error() {
        let html = r#"<div><table class="table table-striped">
            <thead><tr><th>Title</th></tr></thead><tbody></tbody></table></div>"#;
        assert_eq!(parse_announcement_list(html).unwrap().len(), 0);
    }

    #[test]
    fn a_missing_table_is_an_error_not_an_empty_list() {
        // If the markup moves, an agent must not be told there are no announcements.
        let html = "<html><body><p>Something else entirely</p></body></html>";
        assert!(matches!(
            parse_announcement_list(html),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn a_row_without_an_expiry_parses_with_none() {
        let html = r#"<table class="table table-striped"><tbody><tr>
            <td><strong><a class="announcementTitle" href="/announcement/detail/42">No expiry</a></strong></td>
            <td><span>1/2/2026 9:05:00 AM</span></td>
        </tr></tbody></table>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 42);
        assert!(items[0].expires_at.is_none());
    }

    #[test]
    fn a_row_with_an_unreadable_date_is_skipped_not_fatal() {
        // One malformed row must not cost the caller every other announcement.
        let html = r#"<table class="table table-striped"><tbody>
            <tr><td><strong><a class="announcementTitle" href="/announcement/detail/1">Bad</a></strong></td>
                <td><span>not a date</span></td></tr>
            <tr><td><strong><a class="announcementTitle" href="/announcement/detail/2">Good</a></strong></td>
                <td><span>1/2/2026 9:05:00 AM</span></td></tr>
        </tbody></table>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 2);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p condo-api announcements`
Expected: FAIL to compile — `parse_announcement_list`, `AnnouncementSummary`, and
`ParseError` are undefined.

- [ ] **Step 4: Implement `ParseError` and the list parser**

Add to `condo-api/src/error.rs`:

```rust
/// A page's markup did not contain what the parser needed.
///
/// Distinct from `ClientError::Parse`, which covers malformed JSON: this means the HTML
/// arrived intact but its structure moved, so the fix is a new selector, not a retry.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected element not found: {0}")]
    MissingElement(&'static str),
}
```

and give `ClientError` a variant carrying it, so client methods can propagate with `?`:

```rust
    #[error("page structure error: {0}")]
    PageStructure(#[from] ParseError),
```

Re-export `ParseError` from `condo-api/src/lib.rs` alongside `ClientError`.

Prepend to `condo-api/src/model/announcements.rs`:

```rust
use crate::error::ParseError;
use chrono::NaiveDateTime;
use scraper::{Html, Selector};

/// One row of the announcements list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncementSummary {
    pub id: u64,
    pub title: String,
    pub posted_at: NaiveDateTime,
    /// Absent when the announcement has no expiry.
    pub expires_at: Option<NaiveDateTime>,
}

/// Parse Condo Control's US-format local timestamps, e.g. `8/13/2026 4:14:37 PM`.
///
/// Naive local time — the page carries no zone.
pub(crate) fn parse_us_datetime(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%Y %l:%M:%S %p")
        .or_else(|_| NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%Y %I:%M:%S %p"))
        .ok()
}

/// Parse the announcements list page.
///
/// Selectors are anchored on class names because the surrounding page is a full application
/// shell that changes independently of this content. A missing table is an error rather than
/// an empty result: an agent must never be told there are no announcements because the markup
/// moved.
pub fn parse_announcement_list(html: &str) -> Result<Vec<AnnouncementSummary>, ParseError> {
    let doc = Html::parse_fragment(html);
    let table_sel = Selector::parse("table.table-striped").unwrap();
    let row_sel = Selector::parse("tbody tr").unwrap();
    let link_sel = Selector::parse("a.announcementTitle").unwrap();
    let span_sel = Selector::parse("span").unwrap();

    let table = doc
        .select(&table_sel)
        .next()
        .ok_or(ParseError::MissingElement("table.table-striped"))?;

    let mut out = Vec::new();
    for row in table.select(&row_sel) {
        let Some(link) = row.select(&link_sel).next() else {
            continue; // header/footer rows carry no announcement link
        };
        let Some(id) = link
            .value()
            .attr("href")
            .and_then(|h| h.rsplit('/').next())
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        let title = link.text().collect::<String>().trim().to_string();

        let spans: Vec<String> = row
            .select(&span_sel)
            .map(|s| s.text().collect::<String>().trim().to_string())
            .collect();
        // First span is the posted timestamp; an "Expires: …" span may follow.
        let Some(posted_at) = spans.first().and_then(|s| parse_us_datetime(s)) else {
            log::warn!("skipping announcement {id}: unreadable posted date");
            continue;
        };
        let expires_at = spans
            .iter()
            .find_map(|s| s.strip_prefix("Expires:"))
            .and_then(parse_us_datetime);

        out.push(AnnouncementSummary {
            id,
            title,
            posted_at,
            expires_at,
        });
    }
    out.sort_by(|a, b| b.posted_at.cmp(&a.posted_at));
    Ok(out)
}
```

Wire into `condo-api/src/model/mod.rs`:

```rust
pub mod announcements;

pub use announcements::{parse_announcement_list, AnnouncementSummary};
```

and extend the `pub use model::{…}` line in `condo-api/src/lib.rs` with
`parse_announcement_list, AnnouncementSummary`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p condo-api announcements`
Expected: PASS, 6 tests.

- [ ] **Step 6: Write the failing client test**

Create `condo-api/src/client/announcements.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{ClientError, CondoClient, HttpCondoClient};
    use httpmock::prelude::*;

    #[test]
    fn list_announcements_fetches_and_parses_the_page() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/announcements_list.html").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET).path("/announcement/show-list");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let items = client.list_announcements().unwrap();
        m.assert();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, 1541631);
    }

    #[test]
    fn list_announcements_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/announcement/show-list");
            then.status(302).header("location", "/login?NextPage=x");
        });
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.list_announcements(), Err(ClientError::Auth)));
    }

    #[test]
    fn a_page_without_the_table_is_a_structure_error() {
        // The "Invalid address" shell returns HTTP 200, so status alone cannot detect it.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/announcement/show-list");
            then.status(200).body("<html><body>Invalid address</body></html>");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.list_announcements(),
            Err(ClientError::PageStructure(_))
        ));
    }
}
```

- [ ] **Step 7: Run the test to verify it fails**

Run: `cargo test -p condo-api client::announcements`
Expected: FAIL to compile — no `list_announcements` method.

- [ ] **Step 8: Implement the client method**

Prepend to `condo-api/src/client/announcements.rs`:

```rust
use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_announcement_list, AnnouncementSummary};

impl HttpCondoClient {
    /// One attempt at the announcements list page.
    ///
    /// Returns the default view only: currently-posted, non-expired announcements. The
    /// archive is reachable solely through a POST filter form, which this read-only client
    /// will not submit.
    pub(crate) fn list_announcements_once(
        &self,
    ) -> Result<Vec<AnnouncementSummary>, ClientError> {
        let resp = self.http.get(self.url("/announcement/show-list")).send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let html = resp.error_for_status()?.text()?;
        Ok(parse_announcement_list(&html)?)
    }
}
```

In `condo-api/src/client/mod.rs`: add `pub mod announcements;`, add the trait method, and add
the delegating impl.

Trait:

```rust
    /// List currently-posted announcements. The archive is not reachable read-only.
    fn list_announcements(&self) -> Result<Vec<AnnouncementSummary>, ClientError>;
```

Impl:

```rust
    fn list_announcements(&self) -> Result<Vec<AnnouncementSummary>, ClientError> {
        self.with_reauth(|| self.list_announcements_once())
    }
```

Update the `use crate::model::{…}` line to include `AnnouncementSummary`.

**This breaks `condo-fs`'s `MockClient`** in `condo-fs/src/vfs.rs` again — add:

```rust
        fn list_announcements(&self) -> Result<Vec<AnnouncementSummary>, ClientError> {
            unimplemented!("the FUSE layer never lists announcements")
        }
```

with the import it needs. This is the third domain to widen the trait; the follow-up in
`docs/superpowers/plans/2026-08-10-condo-mcp-foundation.md` about splitting `CondoClient` into
per-domain traits is now due. Do not do that split here — record it, finish this plan.

- [ ] **Step 9: Run the client tests to verify they pass**

Run: `cargo test -p condo-api client::announcements`
Expected: PASS, 3 tests.

- [ ] **Step 10: Write the failing MCP integration tests**

Append to `condo-mcp/tests/server.rs`:

```rust
#[tokio::test]
async fn list_announcements_returns_the_current_notices() {
    let server = MockServer::start();
    mock_login(&server);
    let body =
        std::fs::read_to_string("../condo-api/tests/fixtures/announcements_list.html").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/announcement/show-list");
        then.status(200).body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_announcements".into(),
            arguments: None,
        })
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let items = json.as_array().expect("structured content should be an array");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["id"], 1541631);
    assert_eq!(items[0]["title"], "Walk-About Garage Sale - Labor Day Weekend!");
    assert_eq!(items[0]["posted_at"], "2026-08-13T16:14:37");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_moved_announcements_page_is_an_error_not_an_empty_list() {
    let server = MockServer::start();
    mock_login(&server);
    server.mock(|when, then| {
        when.method(GET).path("/announcement/show-list");
        then.status(200).body("<html><body>nothing here</body></html>");
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_announcements".into(),
            arguments: None,
        })
        .await;

    assert!(
        result.is_err() || result.unwrap().is_error == Some(true),
        "a moved page must not read as 'no announcements'"
    );
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty());
    client.cancel().await.unwrap();
}
```

- [ ] **Step 11: Run the tests to verify they fail**

Run: `cargo test -p condo-mcp list_announcements`
Expected: FAIL — the tool is not registered.

- [ ] **Step 12: Implement the tool**

Create `condo-mcp/src/tools/announcements.rs`:

```rust
use crate::server::CondoMcp;
use condo_api::{AnnouncementSummary, CondoClient};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListAnnouncementsParams {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Announcement {
    pub id: u64,
    pub title: String,
    /// Local time, no zone — the association's own clock.
    pub posted_at: String,
    /// Absent when the announcement has no expiry.
    pub expires_at: Option<String>,
}

impl From<AnnouncementSummary> for Announcement {
    fn from(a: AnnouncementSummary) -> Announcement {
        Announcement {
            id: a.id,
            title: a.title,
            posted_at: a.posted_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
            expires_at: a.expires_at.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
        }
    }
}

#[rmcp::tool_router(router = announcements_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_announcements",
        description = "List the condo association's currently-posted announcements, newest \
                       first. Returns each announcement's id, title, posting time, and expiry \
                       if it has one; pass an id to get_announcement for the full text. \
                       IMPORTANT: this returns only announcements that are currently posted \
                       and unexpired. Older announcements exist but are reachable only through \
                       a form this read-only server cannot submit, so absence from this list \
                       does not mean an announcement never existed."
    )]
    pub async fn list_announcements(
        &self,
        Parameters(_): Parameters<ListAnnouncementsParams>,
    ) -> Result<Json<Vec<Announcement>>, ErrorData> {
        let items = self
            .with_client(move |c| c.list_announcements())
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(items.into_iter().map(Announcement::from).collect()))
    }
}
```

Add `pub mod announcements;` to `condo-mcp/src/tools/mod.rs` and merge the router in
`condo-mcp/src/server.rs`'s `build_router`, following however the existing two routers are
merged there.

`ClientError::PageStructure` must map to a useful tool error. Extend `tool_error` in
`condo-mcp/src/server.rs`:

```rust
            ClientError::PageStructure(ref inner) => {
                log::debug!("page structure: {inner}");
                "Condo Control's page layout has changed and this tool can no longer read it. \
                 This needs a code fix, not a retry."
                    .to_string()
            }
```

- [ ] **Step 13: Run the tests to verify they pass**

Run: `cargo test -p condo-mcp`
Expected: PASS — the existing tests plus the 2 new ones.

- [ ] **Step 14: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings \
  && cargo test --all --all-features && ./scripts/guardrails.sh
git add -A
git commit -m "feat(condo-mcp): add the list_announcements tool

Announcements are server-rendered HTML, so this adds the project's first
DOM parser, anchored on class names rather than document position, plus a
ParseError type that distinguishes moved markup from a genuinely empty
list. The archive is reachable only via a POST filter form, so the tool
returns the current view and says so in its description."
```

---

### Task 2: `get_announcement`

**Files:**
- Modify: `condo-api/src/model/announcements.rs`, `condo-api/src/client/announcements.rs`,
  `condo-api/src/client/mod.rs`, `condo-api/src/lib.rs`, `condo-fs/src/vfs.rs`,
  `condo-mcp/src/tools/announcements.rs`, `README.md`
- Test: same three locations as Task 1

**Interfaces:**
- Consumes: everything Task 1 produced.
- Produces:
  - `condo_api::AnnouncementDetail { id: u64, title: String, posted_at: Option<NaiveDateTime>, expires_at: Option<NaiveDateTime>, posted_by: Option<String>, body_markdown: String, attachments: Vec<Attachment> }`
  - `condo_api::Attachment { name: String, uploaded: Option<NaiveDateTime> }`
  - `condo_api::parse_announcement_detail(id: u64, html: &str) -> Result<AnnouncementDetail, ParseError>`
  - `CondoClient::get_announcement(&self, id: u64) -> Result<AnnouncementDetail, ClientError>`
  - MCP tool `get_announcement`, parameters `{ id: u64 }`

- [ ] **Step 1: Write the failing detail-parser tests**

Append to `condo-api/src/model/announcements.rs`'s test module:

```rust
    fn detail_fixture() -> String {
        std::fs::read_to_string("tests/fixtures/announcement_detail.html").unwrap()
    }

    #[test]
    fn parses_the_captured_detail_page() {
        let d = parse_announcement_detail(1541631, &detail_fixture()).unwrap();
        assert_eq!(d.id, 1541631);
        assert_eq!(d.title, "Walk-About Garage Sale - Labor Day Weekend!");
        assert_eq!(d.posted_by.as_deref(), Some("HPMA PRC"));
        assert_eq!(
            d.posted_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(16, 14, 37)
                .unwrap()
        );
        assert_eq!(
            d.expires_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap()
        );
    }

    #[test]
    fn the_body_is_markdown_with_no_html_tags_left() {
        let d = parse_announcement_detail(1541631, &detail_fixture()).unwrap();
        assert!(
            !d.body_markdown.contains("<div") && !d.body_markdown.contains("<figure"),
            "an agent must never receive raw markup: {}",
            d.body_markdown
        );
    }

    #[test]
    fn labels_are_matched_by_text_not_by_position() {
        // Same four labels, reordered and with an extra field inserted. A parser keyed on
        // sibling index would silently mis-assign these.
        let html = r#"<div>
            <label class="control-label">Posted By:</label>
            <div><p class="form-control-static">Someone Else</p></div>
            <label class="control-label">Unrelated:</label>
            <div><p class="form-control-static">ignore me</p></div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Reordered</p></div>
            <div class="ck-content"><p>Body text</p></div>
        </div>"#;
        let d = parse_announcement_detail(7, html).unwrap();
        assert_eq!(d.title, "Reordered");
        assert_eq!(d.posted_by.as_deref(), Some("Someone Else"));
        assert_eq!(d.body_markdown.trim(), "Body text");
    }

    #[test]
    fn a_detail_page_without_a_body_is_an_error() {
        let html = r#"<div><label class="control-label">Title:</label>
            <div><p class="form-control-static">No body</p></div></div>"#;
        assert!(matches!(
            parse_announcement_detail(1, html),
            Err(ParseError::MissingElement("div.ck-content"))
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p condo-api announcements`
Expected: FAIL to compile — `parse_announcement_detail` is undefined.

- [ ] **Step 3: Implement the detail parser**

Add to `condo-api/src/model/announcements.rs`:

```rust
/// One file attached to an announcement. Listed only — the content is not fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub uploaded: Option<NaiveDateTime>,
}

/// A single announcement with its full text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncementDetail {
    pub id: u64,
    pub title: String,
    pub posted_at: Option<NaiveDateTime>,
    pub expires_at: Option<NaiveDateTime>,
    pub posted_by: Option<String>,
    pub body_markdown: String,
    pub attachments: Vec<Attachment>,
}

/// Read the value that follows a `label.control-label` whose text is `label_text`.
///
/// Matched by label text rather than by position: the detail page reorders and omits fields
/// depending on how the announcement was created.
fn labelled_value(doc: &Html, label_text: &str) -> Option<String> {
    let label_sel = Selector::parse("label.control-label").unwrap();
    let value_sel = Selector::parse("p.form-control-static").unwrap();
    let labels: Vec<_> = doc.select(&label_sel).collect();
    let values: Vec<_> = doc.select(&value_sel).collect();
    // Values appear in the same document order as their labels, so the Nth matching label
    // corresponds to the Nth value element.
    let idx = labels
        .iter()
        .position(|l| l.text().collect::<String>().trim() == label_text)?;
    values
        .get(idx)
        .map(|v| v.text().collect::<String>().trim().to_string())
}

/// Parse an announcement detail page.
///
/// `id` is passed in rather than scraped: the caller already knows it, and the page does not
/// display it.
pub fn parse_announcement_detail(
    id: u64,
    html: &str,
) -> Result<AnnouncementDetail, ParseError> {
    let doc = Html::parse_fragment(html);

    let body_sel = Selector::parse("div.ck-content").unwrap();
    let body_el = doc
        .select(&body_sel)
        .next()
        .ok_or(ParseError::MissingElement("div.ck-content"))?;
    let body_markdown = htmd::convert(&body_el.html()).unwrap_or_default();

    let attach_sel = Selector::parse("table.table-striped tbody tr").unwrap();
    let cell_sel = Selector::parse("td").unwrap();
    let attachments = doc
        .select(&attach_sel)
        .filter_map(|row| {
            let cells: Vec<String> = row
                .select(&cell_sel)
                .map(|c| c.text().collect::<String>().trim().to_string())
                .collect();
            // Columns are Options | File Name | Date Uploaded.
            let name = cells.get(1)?.clone();
            if name.is_empty() {
                return None;
            }
            Some(Attachment {
                name,
                uploaded: cells.get(2).and_then(|s| parse_us_datetime(s)),
            })
        })
        .collect();

    Ok(AnnouncementDetail {
        id,
        title: labelled_value(&doc, "Title:").unwrap_or_default(),
        posted_at: labelled_value(&doc, "Posted:").and_then(|s| parse_us_datetime(&s)),
        expires_at: labelled_value(&doc, "Expires:").and_then(|s| parse_us_datetime(&s)),
        posted_by: labelled_value(&doc, "Posted By:"),
        body_markdown,
        attachments,
    })
}
```

Export `parse_announcement_detail`, `AnnouncementDetail`, and `Attachment` from
`model/mod.rs` and `lib.rs` alongside the Task 1 exports.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p condo-api announcements`
Expected: PASS, 10 tests.

- [ ] **Step 5: Write the failing client test**

Append to `condo-api/src/client/announcements.rs`'s test module:

```rust
    #[test]
    fn get_announcement_fetches_the_detail_page_by_id() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/announcement_detail.html").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET).path("/announcement/detail/1541631");
            then.status(200).body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let d = client.get_announcement(1541631).unwrap();
        m.assert();
        assert_eq!(d.id, 1541631);
        assert_eq!(d.title, "Walk-About Garage Sale - Labor Day Weekend!");
    }

    #[test]
    fn get_announcement_404_is_a_not_found_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/announcement/detail/999");
            then.status(404);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.get_announcement(999),
            Err(ClientError::NotFound)
        ));
    }
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `cargo test -p condo-api client::announcements`
Expected: FAIL to compile — no `get_announcement` method.

- [ ] **Step 7: Implement the client method**

Add to `condo-api/src/client/announcements.rs`:

```rust
impl HttpCondoClient {
    /// One attempt at a single announcement's detail page.
    pub(crate) fn get_announcement_once(
        &self,
        id: u64,
    ) -> Result<AnnouncementDetail, ClientError> {
        let resp = self
            .http
            .get(self.url(&format!("/announcement/detail/{id}")))
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ClientError::NotFound);
        }
        let html = resp.error_for_status()?.text()?;
        Ok(parse_announcement_detail(id, &html)?)
    }
}
```

Add the trait method and delegating impl in `client/mod.rs` exactly as in Task 1
(`with_reauth(|| self.get_announcement_once(id))`), and add the corresponding
`unimplemented!()` stub to `condo-fs`'s `MockClient`.

- [ ] **Step 8: Run the client tests to verify they pass**

Run: `cargo test -p condo-api client::announcements`
Expected: PASS, 5 tests.

- [ ] **Step 9: Write the failing MCP integration test**

Append to `condo-mcp/tests/server.rs`:

```rust
#[tokio::test]
async fn get_announcement_returns_the_full_text_as_markdown() {
    let server = MockServer::start();
    mock_login(&server);
    let body =
        std::fs::read_to_string("../condo-api/tests/fixtures/announcement_detail.html").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/announcement/detail/1541631");
        then.status(200).body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "get_announcement".into(),
            arguments: serde_json::json!({ "id": 1541631 }).as_object().cloned(),
        })
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    assert_eq!(json["id"], 1541631);
    assert_eq!(json["title"], "Walk-About Garage Sale - Labor Day Weekend!");
    assert_eq!(json["posted_by"], "HPMA PRC");
    let body_md = json["body"].as_str().expect("body should be a string");
    assert!(!body_md.contains("<div"), "body must be markdown, got: {body_md}");
    client.cancel().await.unwrap();
}
```

- [ ] **Step 10: Run the test to verify it fails**

Run: `cargo test -p condo-mcp get_announcement`
Expected: FAIL — the tool is not registered.

- [ ] **Step 11: Implement the tool**

Add to `condo-mcp/src/tools/announcements.rs`:

```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetAnnouncementParams {
    /// Announcement id, as returned by list_announcements.
    pub id: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AnnouncementFull {
    pub id: u64,
    pub title: String,
    pub posted_at: Option<String>,
    pub expires_at: Option<String>,
    /// Who posted it — often a committee or role rather than a person.
    pub posted_by: Option<String>,
    /// The announcement text, converted from the site's HTML to Markdown.
    pub body: String,
    pub attachments: Vec<AnnouncementAttachment>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AnnouncementAttachment {
    pub name: String,
    pub uploaded: Option<String>,
}
```

with a `From<AnnouncementDetail>` conversion formatting the timestamps the same way Task 1
does, and the tool method inside the same `#[rmcp::tool_router(router = announcements_router …)]`
impl block:

```rust
    #[tool(
        name = "get_announcement",
        description = "Get one condo association announcement in full, by the id returned \
                       from list_announcements. Returns the title, who posted it, when it was \
                       posted and expires, and the full text converted to Markdown. \
                       Attachments are listed by name only — their contents cannot be \
                       downloaded, so do not report them as unreadable files."
    )]
    pub async fn get_announcement(
        &self,
        Parameters(params): Parameters<GetAnnouncementParams>,
    ) -> Result<Json<AnnouncementFull>, ErrorData> {
        let id = params.id;
        let detail = self
            .with_client(move |c| c.get_announcement(id))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(AnnouncementFull::from(detail)))
    }
```

- [ ] **Step 12: Run the tests to verify they pass**

Run: `cargo test -p condo-mcp`
Expected: PASS, all existing plus the new test.

- [ ] **Step 13: Run the full gate**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all --all-features && ./scripts/guardrails.sh`
Expected: all clean.

- [ ] **Step 14: Document both tools**

Add `list_announcements` and `get_announcement` to the `condo-mcp` section of `README.md`,
stating plainly that only current, unexpired announcements are reachable and why.

- [ ] **Step 15: Commit**

```bash
git add -A
git commit -m "feat(condo-mcp): add the get_announcement tool

Parses the announcement detail page: labelled metadata matched by label
text rather than document position, CKEditor body converted to Markdown,
and attachments listed by name. Attachment contents remain unreachable,
which the tool description states so an agent does not report them as
unreadable files."
```
