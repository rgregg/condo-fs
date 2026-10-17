# condo-mcp Calendar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `list_events` MCP tool returning amenity bookings from the association's calendar over a date range.

**Architecture:** Follows the pattern the foundation branch established — `model/calendar.rs` parses, `client/calendar.rs` fetches, `tools/calendar.rs` exposes, and the `CondoClient` trait grows one method that `client/mod.rs`'s trait impl delegates to a `*_once` helper wrapped in `with_reauth`.

**Tech Stack:** Rust 2021 (1.95), blocking `reqwest`, `rmcp` 3, `chrono` for date handling, `httpmock` for tests.

**Source spec:** `docs/superpowers/specs/2026-08-10-condo-mcp-design.md` (Calendar section, revised after discovery)

## Discovery findings this plan is built on

Established by probing the live site on 2026-08-10:

- **Endpoint:** `GET /amenity/json-source/?filter=&start=<unix>&end=<unix>`, with the
  `X-Requested-With: XMLHttpRequest` header. `start`/`end` are **Unix seconds**. Omitting
  them, or passing ISO dates, 302s to `/Error/invalid-address`.
- **Response:** a bare JSON array. Each element:
  `{ id: number, title: string, start: "2025-08-24T14:00:00", end: "2025-08-24T19:00:00", allDay: bool, color: string, textColor: string, isEventOwner: bool, isLinkedBooking: bool, isHoliday: bool }`.
  `start`/`end` are naive local-time ISO strings with no zone.
- **`end` may be null** for a zero-length or all-day entry — treat it as optional.
- Titles name the facility, never the resident (`"Indian Cove Marina Guest Dock #1 booked"`).
- A real captured response is committed at `condo-api/tests/fixtures/amenity_events.json`.
- `get_event` is not implementable: `CanViewDetailsOfAnyBooking = 'False'` for a resident
  account. Do not build it.

## Global Constraints

- **Read-only.** GET only. `scripts/guardrails.sh` enforces this in CI and must keep passing.
- Rust edition 2021, `rust-version = "1.95"`, license MIT.
- `condo-api` stays **blocking**; `condo-mcp` bridges with `spawn_blocking`.
- The password is never logged, never in an error message, never serialized.
- In `condo-mcp`, stdout is the MCP wire protocol; logging goes to stderr.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  and `cargo test --all --all-features` must pass.
- Tool descriptions are read by agents deciding whether to call them; they must state what
  is returned and its limits.

---

### Task 1: `list_events` end to end

One task: the domain is one endpoint and one tool, and splitting the parser from the tool
would produce two half-testable deliverables.

**Files:**
- Create: `condo-api/src/model/calendar.rs`, `condo-api/src/client/calendar.rs`,
  `condo-mcp/src/tools/calendar.rs`
- Modify: `condo-api/src/model/mod.rs`, `condo-api/src/client/mod.rs`,
  `condo-api/src/lib.rs`, `condo-api/Cargo.toml`, `condo-mcp/src/tools/mod.rs`,
  `condo-mcp/src/server.rs`, `condo-mcp/Cargo.toml`, `README.md`
- Test: unit tests in `condo-api/src/model/calendar.rs` and `condo-api/src/client/calendar.rs`;
  integration tests in `condo-mcp/tests/server.rs`
- Fixture (already committed): `condo-api/tests/fixtures/amenity_events.json`

**Interfaces:**
- Consumes: `HttpCondoClient`'s `pub(crate)` `http`, `url`, `with_reauth`; the
  `CondoClient` trait; `ClientError`; `CondoMcp::with_client` and `CondoMcp::tool_error`;
  `CondoMcp::build_router`.
- Produces:
  - `condo_api::Event { id: u64, title: String, start: NaiveDateTime, end: Option<NaiveDateTime>, all_day: bool, is_holiday: bool, is_mine: bool }`
  - `condo_api::parse_event_list(json: &str) -> Result<Vec<Event>, serde_json::Error>`
  - `CondoClient::list_events(&self, start: i64, end: i64) -> Result<Vec<Event>, ClientError>` (Unix seconds)
  - MCP tool `list_events`, parameters `{ start?: "YYYY-MM-DD", end?: "YYYY-MM-DD" }`

- [ ] **Step 1: Add `chrono` to both crates**

In the root `Cargo.toml` `[workspace.dependencies]`:

```toml
chrono = { version = "0.4", default-features = false, features = ["std", "serde", "clock"] }
```

Add `chrono = { workspace = true }` to `condo-api/Cargo.toml` and `condo-mcp/Cargo.toml`
`[dependencies]`.

- [ ] **Step 2: Write the failing model test**

Create `condo-api/src/model/calendar.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_captured_amenity_feed() {
        let json = std::fs::read_to_string("tests/fixtures/amenity_events.json").unwrap();
        let events = parse_event_list(&json).unwrap();
        assert_eq!(events.len(), 4);
        let first = &events[0];
        assert_eq!(first.id, 7543190);
        assert_eq!(first.title, "All of North Beach Shelter booked");
        assert_eq!(
            first.start,
            chrono::NaiveDate::from_ymd_opt(2025, 8, 24)
                .unwrap()
                .and_hms_opt(14, 0, 0)
                .unwrap()
        );
        assert_eq!(
            first.end.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2025, 8, 24)
                .unwrap()
                .and_hms_opt(19, 0, 0)
                .unwrap()
        );
        assert!(!first.all_day);
        assert!(!first.is_holiday);
        assert!(!first.is_mine);
    }

    #[test]
    fn a_null_end_is_allowed() {
        // Observed for zero-length and all-day entries; must not fail the whole parse.
        let json = r#"[{"id":1,"title":"Holiday","start":"2026-01-01T00:00:00","end":null,
            "allDay":true,"color":"#fff","textColor":"#000","isEventOwner":false,
            "isLinkedBooking":false,"isHoliday":true}]"#;
        let events = parse_event_list(json).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].end.is_none());
        assert!(events[0].all_day);
        assert!(events[0].is_holiday);
    }

    #[test]
    fn an_empty_feed_parses_to_no_events() {
        assert_eq!(parse_event_list("[]").unwrap().len(), 0);
    }

    #[test]
    fn events_come_back_sorted_by_start() {
        let json = r#"[
            {"id":2,"title":"Later","start":"2026-03-02T10:00:00","end":null,"allDay":false,
             "color":"","textColor":"","isEventOwner":false,"isLinkedBooking":false,"isHoliday":false},
            {"id":1,"title":"Earlier","start":"2026-03-01T10:00:00","end":null,"allDay":false,
             "color":"","textColor":"","isEventOwner":false,"isLinkedBooking":false,"isHoliday":false}
        ]"#;
        let events = parse_event_list(json).unwrap();
        assert_eq!(events[0].title, "Earlier", "the feed is not sorted; the parser must sort");
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p condo-api calendar`
Expected: FAIL to compile — `parse_event_list` and `Event` are undefined.

- [ ] **Step 4: Implement the model**

Prepend to `condo-api/src/model/calendar.rs`:

```rust
use chrono::NaiveDateTime;
use serde::Deserialize;

/// One amenity booking from the association's calendar.
///
/// Timestamps are naive local time — the feed carries no zone. Callers that need an
/// absolute instant must apply the association's timezone themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: u64,
    pub title: String,
    pub start: NaiveDateTime,
    /// Absent for zero-length and some all-day entries.
    pub end: Option<NaiveDateTime>,
    pub all_day: bool,
    pub is_holiday: bool,
    /// True when the booking belongs to the authenticated user.
    pub is_mine: bool,
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(rename = "id")]
    id: u64,
    #[serde(rename = "title")]
    title: String,
    #[serde(rename = "start")]
    start: NaiveDateTime,
    #[serde(rename = "end")]
    end: Option<NaiveDateTime>,
    #[serde(rename = "allDay")]
    all_day: bool,
    #[serde(rename = "isEventOwner")]
    is_event_owner: bool,
    #[serde(rename = "isHoliday")]
    is_holiday: bool,
}

/// Parse the amenity calendar feed, sorted by start time.
///
/// `color`, `textColor`, and `isLinkedBooking` are dropped: the first two are display
/// concerns and the third is an internal flag with no meaning to a caller.
pub fn parse_event_list(json: &str) -> Result<Vec<Event>, serde_json::Error> {
    let rows: Vec<RawEvent> = serde_json::from_str(json)?;
    let mut events: Vec<Event> = rows
        .into_iter()
        .map(|r| Event {
            id: r.id,
            title: r.title,
            start: r.start,
            end: r.end,
            all_day: r.all_day,
            is_holiday: r.is_holiday,
            is_mine: r.is_event_owner,
        })
        .collect();
    // The feed arrives in no particular order; callers summarizing "what's coming up"
    // depend on start order, so sorting is the parser's job.
    events.sort_by_key(|e| e.start);
    Ok(events)
}
```

Wire it up in `condo-api/src/model/mod.rs`:

```rust
pub mod calendar;
pub mod library;

pub use calendar::{parse_event_list, Event};
pub use library::{parse_file_list, Entry, FileMeta};
```

and extend the re-export in `condo-api/src/lib.rs`:

```rust
pub use model::{parse_event_list, parse_file_list, Entry, Event, FileMeta};
```

- [ ] **Step 5: Run the model tests to verify they pass**

Run: `cargo test -p condo-api calendar`
Expected: PASS, 4 tests.

- [ ] **Step 6: Write the failing client test**

Create `condo-api/src/client/calendar.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{ClientError, CondoClient, HttpCondoClient};
    use httpmock::prelude::*;

    #[test]
    fn list_events_sends_unix_seconds_and_parses_the_feed() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/amenity_events.json").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/amenity/json-source/")
                .query_param("filter", "")
                .query_param("start", "1754006400")
                .query_param("end", "1756684800")
                .header("x-requested-with", "XMLHttpRequest");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let events = client.list_events(1754006400, 1756684800).unwrap();
        m.assert();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].id, 7543190);
    }

    #[test]
    fn list_events_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/amenity/json-source/");
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
        assert!(matches!(
            client.list_events(0, 1),
            Err(ClientError::Auth)
        ));
    }

    #[test]
    fn list_events_reauths_once_then_gives_up() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(GET).path("/amenity/json-source/");
            then.status(302).header("location", "/login");
        });
        let login_hits = server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.list_events(0, 1), Err(ClientError::Auth)));
        assert_eq!(login_hits.hits(), 1, "should re-auth exactly once then give up");
    }
}
```

- [ ] **Step 7: Run the test to verify it fails**

Run: `cargo test -p condo-api client::calendar`
Expected: FAIL to compile — no method `list_events` on `HttpCondoClient`.

- [ ] **Step 8: Implement the client method**

Prepend to `condo-api/src/client/calendar.rs`:

```rust
use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_event_list, Event};

impl HttpCondoClient {
    /// One attempt at the amenity calendar feed. `start`/`end` are Unix seconds — the
    /// endpoint 302s to an error page for ISO dates or for a missing range.
    pub(crate) fn list_events_once(
        &self,
        start: i64,
        end: i64,
    ) -> Result<Vec<Event>, ClientError> {
        let resp = self
            .http
            .get(self.url("/amenity/json-source/"))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(&[
                ("filter", String::new()),
                ("start", start.to_string()),
                ("end", end.to_string()),
            ])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let text = resp.error_for_status()?.text()?;
        Ok(parse_event_list(&text)?)
    }
}
```

In `condo-api/src/client/mod.rs`: add `pub mod calendar;` beside `pub mod library;`, add the
trait method, and add the delegating impl.

Trait addition:

```rust
    /// List amenity bookings between two instants, given as Unix seconds.
    fn list_events(&self, start: i64, end: i64) -> Result<Vec<Event>, ClientError>;
```

Impl addition inside `impl CondoClient for HttpCondoClient`:

```rust
    fn list_events(&self, start: i64, end: i64) -> Result<Vec<Event>, ClientError> {
        self.with_reauth(|| self.list_events_once(start, end))
    }
```

The `use` line at the top of `client/mod.rs` becomes:

```rust
use crate::model::{Entry, Event, FileMeta};
```

**This breaks `condo-fs`:** its hand-written `MockClient` in `condo-fs/src/vfs.rs` implements
`CondoClient` and must gain the new method. Add to that impl:

```rust
        fn list_events(&self, _start: i64, _end: i64) -> Result<Vec<Event>, ClientError> {
            unimplemented!("the FUSE layer never lists calendar events")
        }
```

with whatever import it needs. This is the trait-scaling problem the foundation plan's
follow-ups section predicted; it is one method today, so absorb it here and split the trait
in a later plan rather than expanding this one.

- [ ] **Step 9: Run the client tests to verify they pass**

Run: `cargo test -p condo-api client::calendar`
Expected: PASS, 3 tests.

- [ ] **Step 10: Write the failing MCP integration tests**

Append to `condo-mcp/tests/server.rs`:

```rust
#[tokio::test]
async fn list_events_returns_bookings_in_the_range() {
    let server = MockServer::start();
    mock_login(&server);
    let body = std::fs::read_to_string("../condo-api/tests/fixtures/amenity_events.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/amenity/json-source/");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_events".into(),
            arguments: serde_json::json!({ "start": "2025-08-01", "end": "2025-09-30" })
                .as_object()
                .cloned(),
        })
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let events = json.as_array().expect("structured content should be an array");
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["id"], 7543190);
    assert_eq!(events[0]["title"], "All of North Beach Shelter booked");
    assert_eq!(events[0]["start"], "2025-08-24T14:00:00");
    assert_eq!(events[0]["all_day"], false);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_events_converts_dates_to_unix_seconds() {
    // The endpoint 302s on ISO dates, so the conversion is load-bearing, not cosmetic.
    let server = MockServer::start();
    mock_login(&server);
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/amenity/json-source/")
            .query_param("start", "1754006400")
            .query_param("end", "1756598400");
        then.status(200).body("[]");
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_events".into(),
            arguments: serde_json::json!({ "start": "2025-08-01", "end": "2025-08-31" })
                .as_object()
                .cloned(),
        })
        .await
        .unwrap();

    m.assert();
    assert_ne!(result.is_error, Some(true));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_unparseable_date_is_a_tool_error_not_a_panic() {
    let server = MockServer::start();
    mock_login(&server);
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParam {
            name: "list_events".into(),
            arguments: serde_json::json!({ "start": "last Tuesday" }).as_object().cloned(),
        })
        .await;

    assert!(
        result.is_err() || result.unwrap().is_error == Some(true),
        "a bad date must be reported, not accepted"
    );
    // The server must still be answering.
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty());
    client.cancel().await.unwrap();
}
```

- [ ] **Step 11: Run the tests to verify they fail**

Run: `cargo test -p condo-mcp list_events`
Expected: FAIL — the tool is not registered, so the calls return a "tool not found" error.

- [ ] **Step 12: Implement the tool**

Create `condo-mcp/src/tools/calendar.rs`:

```rust
use crate::server::CondoMcp;
use chrono::{Duration, NaiveDate, NaiveDateTime, Utc};
use condo_api::{CondoClient, Event};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListEventsParams {
    /// Inclusive start of the range, as YYYY-MM-DD. Defaults to today.
    pub start: Option<String>,
    /// Exclusive end of the range, as YYYY-MM-DD. Defaults to 60 days after today.
    pub end: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CalendarEvent {
    pub id: u64,
    /// Names the facility booked, never the resident who booked it.
    pub title: String,
    /// Local time, no zone — the association's own clock.
    pub start: String,
    /// Absent for zero-length and some all-day entries.
    pub end: Option<String>,
    pub all_day: bool,
    pub is_holiday: bool,
    /// True when the booking belongs to the authenticated user.
    pub is_mine: bool,
}

impl From<Event> for CalendarEvent {
    fn from(e: Event) -> CalendarEvent {
        CalendarEvent {
            id: e.id,
            title: e.title,
            start: e.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            end: e.end.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
            all_day: e.all_day,
            is_holiday: e.is_holiday,
            is_mine: e.is_mine,
        }
    }
}

/// Parse a YYYY-MM-DD date into a Unix second count at midnight.
fn parse_day(s: &str) -> Result<i64, ErrorData> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| d.and_hms_opt(0, 0, 0).expect("midnight is always valid").and_utc().timestamp())
        .map_err(|_| {
            ErrorData::invalid_params(
                format!("Could not read {s:?} as a date. Use YYYY-MM-DD, e.g. 2026-08-01."),
                None,
            )
        })
}

#[rmcp::tool_router(router = calendar_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_events",
        description = "List bookings on the condo association's amenity calendar between two \
                       dates — reservations of shared facilities such as shelters, docks, and \
                       the clubhouse. Dates are YYYY-MM-DD and default to today through 60 days \
                       out. Results are sorted by start time. Titles name the facility booked, \
                       not the resident who booked it, and booking details are not available."
    )]
    pub async fn list_events(
        &self,
        Parameters(params): Parameters<ListEventsParams>,
    ) -> Result<Json<Vec<CalendarEvent>>, ErrorData> {
        let today: NaiveDateTime = Utc::now().naive_utc().date().and_hms_opt(0, 0, 0).unwrap();
        let start = match params.start {
            Some(ref s) => parse_day(s)?,
            None => today.and_utc().timestamp(),
        };
        let end = match params.end {
            Some(ref s) => parse_day(s)?,
            None => (today + Duration::days(60)).and_utc().timestamp(),
        };
        if end < start {
            return Err(ErrorData::invalid_params(
                "The end date is before the start date.".to_string(),
                None,
            ));
        }
        let events = self
            .with_client(move |c| c.list_events(start, end))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(events.into_iter().map(CalendarEvent::from).collect()))
    }
}
```

Add `pub mod calendar;` to `condo-mcp/src/tools/mod.rs`, and merge the router in
`condo-mcp/src/server.rs`:

```rust
    fn build_router() -> ToolRouter<Self> {
        Self::library_router() + Self::calendar_router()
    }
```

If `ToolRouter` does not implement `Add`, use whatever merge API `rmcp` 3.1.2 provides
(check `cargo doc -p rmcp`); the `list_events` test in Step 10 is what proves the merge
worked.

- [ ] **Step 13: Run the tests to verify they pass**

Run: `cargo test -p condo-mcp`
Expected: PASS, 10 tests (7 existing + 3 new).

- [ ] **Step 14: Run the full gate**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all --all-features && ./scripts/guardrails.sh`
Expected: all clean. The guardrail must still report no mutating HTTP calls.

- [ ] **Step 15: Document the tool**

Add `list_events` to the `condo-mcp` section of `README.md` beside `list_folder`: what it
returns, that dates are `YYYY-MM-DD`, the defaults, and that it covers amenity bookings
rather than a general events calendar.

- [ ] **Step 16: Commit**

```bash
git add -A
git commit -m "feat(condo-mcp): add the list_events tool over the amenity calendar

Adds the amenity booking feed (/amenity/json-source/, Unix-second range
params) as a model, a client method, and a list_events MCP tool. Discovery
established that Condo Control has no general events calendar and that
booking details are permission-gated, so get_event is not implementable
and was cut from the design."
```

---

## Correction (2026-08-14)

This plan's premise — repeated in its commit message above — that "Condo Control has no
general events calendar" is **wrong**. It reflected having found only the amenity module.
There is a separate events module: `GET /event/get-events-dashboard/` returns the community
calendar (board meetings, committee meetings, classes), 2,031 events spanning 2022–2030,
each with a detail link at `/event/event-details/{id}`. It ignores date parameters and
returns the whole set (~552 KB), so filtering must be client-side.

Everything this plan built is still correct — `list_events` genuinely covers amenity
bookings, and `get_event` genuinely is not implementable for them. What is wrong is the
claim that amenity bookings are the *only* calendar. Community events remain unbuilt and are
the more useful source for board work.

## Follow-up carried out of this plan

**Long bookings starting well before the requested range may be missed.** The overlap filter
can only keep what the server returns, and the fetch window reaches just one day before
`start_date`. If the endpoint selects events by their *start* rather than by overlap, a
week-long reservation that began before that window will never be fetched — the filter cannot
recover it. Whether `/amenity/json-source/` selects by start or by overlap is not knowable
from the captured fixture and was not probed. Resolve it with a targeted probe (request a
narrow range that falls inside a known multi-day booking and see whether it comes back) before
widening the start bound further, since widening trades against response size.
