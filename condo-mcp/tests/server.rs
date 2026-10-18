use condo_api::Credentials;
use condo_mcp::server::CondoMcp;
use httpmock::prelude::*;
use rmcp::model::CallToolRequestParams;
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

async fn connect(
    server: &MockServer,
    root: u64,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    // reqwest's blocking client builds its own internal Tokio runtime; constructing it
    // directly inside this async test body would nest runtimes and panic on drop. Build
    // it on a spawn_blocking thread, same as the server does for every client call.
    let base_url = server.base_url();
    let mcp = tokio::task::spawn_blocking(move || {
        CondoMcp::new(base_url, creds(), root, "<test>".into())
    })
    .await
    .unwrap()
    .unwrap();
    serve(mcp).await
}

/// Connect to a server whose downloads go to `dir` and whose attachment storage may be the
/// local (plain-http) mock server.
async fn connect_with_download_dir(
    server: &MockServer,
    dir: std::path::PathBuf,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let base_url = server.base_url();
    let mcp = tokio::task::spawn_blocking(move || {
        CondoMcp::new(base_url, creds(), 137473, "<test>".into())
            .unwrap()
            .with_download_dir(dir)
            .with_storage_host_check(|h| h == "127.0.0.1")
    })
    .await
    .unwrap();
    serve(mcp).await
}

async fn serve(mcp: CondoMcp) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
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

    let t = tools
        .iter()
        .find(|t| t.name.as_ref() == "list_folder")
        .unwrap();
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
        .call_tool(
            CallToolRequestParams::new("list_folder").with_arguments(
                serde_json::json!({ "folder_id": 262667 })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    assert_ne!(
        result.is_error,
        Some(true),
        "tool returned an error: {result:?}"
    );
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let entries = json
        .as_array()
        .expect("structured content should be an array");
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
        .call_tool(CallToolRequestParams::new("list_folder"))
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
        .call_tool(CallToolRequestParams::new("list_folder"))
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
        .call_tool(CallToolRequestParams::new("list_folder"))
        .await;
    assert!(first.is_err() || first.unwrap().is_error == Some(true));

    // The server must still be answering after a tool failure.
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty(), "server died after a failing tool call");
    client.cancel().await.unwrap();
}

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
        .call_tool(
            CallToolRequestParams::new("list_events").with_arguments(
                serde_json::json!({ "start": "2025-08-01", "end": "2025-09-30" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let events = json
        .as_array()
        .expect("structured content should be an array");
    assert_eq!(events.len(), 4);
    // The parser sorts by start; the fixture's chronologically-earliest entry is
    // id 7570058 at 2025-08-02T18:00:00, not the fixture's first line.
    assert_eq!(events[0]["id"], 7570058);
    assert_eq!(events[0]["title"], "Half of North Beach Shelter booked");
    assert_eq!(events[0]["start"], "2025-08-02T18:00:00");
    assert_eq!(events[0]["all_day"], false);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_events_converts_dates_to_unix_seconds() {
    // The endpoint 302s on ISO dates, so the conversion is load-bearing, not cosmetic.
    // The outbound query is widened asymmetrically around the requested [start, end]
    // range: the feed's own timestamps are naive association-local time with no zone, so
    // a query built directly from UTC-midnight Unix seconds could miss in-range bookings
    // depending on the association's UTC offset. One day back covers the start edge at
    // every real-world offset (UTC-12..UTC+14); the end edge needs two days forward, not
    // one, to reach past local midnight at the worst-case offset (UTC-12) — see the
    // derivation in list_events' doc comment. 2025-08-01 widens to 2025-07-31T00:00:00Z
    // (1753920000) and 2025-08-31 widens to 2025-09-02T00:00:00Z (1756771200); the tool
    // then filters the parsed events back down to the exact requested days (see
    // list_events_includes_the_full_end_day_and_excludes_outside_the_range).
    let server = MockServer::start();
    mock_login(&server);
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/amenity/json-source/")
            .query_param("start", "1753920000")
            .query_param("end", "1756771200");
        then.status(200).body("[]");
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(
            CallToolRequestParams::new("list_events").with_arguments(
                serde_json::json!({ "start": "2025-08-01", "end": "2025-08-31" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    m.assert();
    assert_ne!(result.is_error, Some(true));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_events_includes_the_full_end_day_and_excludes_outside_the_range() {
    // The endpoint's feed carries naive association-local timestamps with no zone, so the
    // tool widens its outbound query (one day back, two days forward — proven by
    // list_events_converts_dates_to_unix_seconds) and then filters the parsed results by
    // comparing each event's naive date against the requested [start, end] range,
    // inclusive on both ends. This proves the filter half of that mechanism: an event on
    // the evening of the *end* date must survive (it used to be silently dropped by a
    // UTC-midnight cutoff), while events that only fall inside the widened fetch window —
    // before start, or after end — must not leak into the result. The query_param
    // matchers below pin the fetch window itself, so this test would fail (not just the
    // params test) if the widening regressed.
    let server = MockServer::start();
    mock_login(&server);
    let body = serde_json::json!([
        {
            "id": 1, "title": "Just before the requested range", "start": "2025-07-31T20:00:00",
            "end": null, "allDay": false, "color": "", "textColor": "",
            "isEventOwner": false, "isLinkedBooking": false, "isHoliday": false
        },
        {
            "id": 2, "title": "Evening of the end date", "start": "2025-08-31T19:00:00",
            "end": null, "allDay": false, "color": "", "textColor": "",
            "isEventOwner": false, "isLinkedBooking": false, "isHoliday": false
        },
        {
            "id": 3, "title": "Just after the requested range", "start": "2025-09-02T01:00:00",
            "end": null, "allDay": false, "color": "", "textColor": "",
            "isEventOwner": false, "isLinkedBooking": false, "isHoliday": false
        }
    ])
    .to_string();
    server.mock(|when, then| {
        when.method(GET)
            .path("/amenity/json-source/")
            .query_param("start", "1753920000")
            .query_param("end", "1756771200");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(
            CallToolRequestParams::new("list_events").with_arguments(
                serde_json::json!({ "start": "2025-08-01", "end": "2025-08-31" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let events = json
        .as_array()
        .expect("structured content should be an array");
    let ids: Vec<i64> = events.iter().map(|e| e["id"].as_i64().unwrap()).collect();
    assert_eq!(
        ids,
        vec![2],
        "only the end-of-range evening booking should survive: it must not be silently \
         dropped, and the day-before/day-after entries fetched by the widened query must \
         be filtered back out"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_events_returns_a_multi_day_booking_that_only_partly_overlaps_the_range() {
    // The committed fixture's id 7504669 runs 2025-08-08T18:00 -> 2025-08-10T18:00. A
    // request covering only the middle/tail of that booking (not its start) must still
    // return it: filtering on the event's start date alone would wrongly drop a booking
    // that started before the requested range but overlaps it.
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
        .call_tool(
            CallToolRequestParams::new("list_events").with_arguments(
                serde_json::json!({ "start": "2025-08-09", "end": "2025-08-10" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let events = json
        .as_array()
        .expect("structured content should be an array");
    let ids: Vec<i64> = events.iter().map(|e| e["id"].as_i64().unwrap()).collect();
    assert_eq!(
        ids,
        vec![7504669],
        "a booking that started before the requested range but overlaps it must be \
         returned, not dropped by a start-date-only filter"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_events_accepts_a_single_day_range() {
    // start == end is a well-formed one-day request now that both ends are documented
    // inclusive; it used to be wrongly rejected under the old exclusive-end model. This
    // is also the narrowest possible range, so it's the most likely to expose an
    // off-by-one in the widen/filter arithmetic.
    let server = MockServer::start();
    mock_login(&server);
    let body = serde_json::json!([
        {
            "id": 1, "title": "On the requested day", "start": "2025-08-31T10:00:00",
            "end": null, "allDay": false, "color": "", "textColor": "",
            "isEventOwner": false, "isLinkedBooking": false, "isHoliday": false
        }
    ])
    .to_string();
    server.mock(|when, then| {
        when.method(GET).path("/amenity/json-source/");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(
            CallToolRequestParams::new("list_events").with_arguments(
                serde_json::json!({ "start": "2025-08-31", "end": "2025-08-31" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let events = json
        .as_array()
        .expect("structured content should be an array");
    assert_eq!(
        events.len(),
        1,
        "a single-day range must not be rejected as invalid"
    );
    assert_eq!(events[0]["id"], 1);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_unparseable_date_is_a_tool_error_not_a_panic() {
    let server = MockServer::start();
    mock_login(&server);
    // If a bad date were (wrongly) accepted, this is what the tool would call next; by
    // asserting zero hits below, the test proves the date was rejected *before* any
    // network call, not merely that a resulting request happened to fail.
    let m = server.mock(|when, then| {
        when.method(GET).path("/amenity/json-source/");
        then.status(200).body("[]");
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(
            CallToolRequestParams::new("list_events").with_arguments(
                serde_json::json!({ "start": "last Tuesday" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await;

    assert!(
        result.is_err() || result.unwrap().is_error == Some(true),
        "a bad date must be reported, not accepted"
    );
    assert_eq!(
        m.hits(),
        0,
        "a bad date must be rejected before any network call"
    );
    // The server must still be answering.
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty());
    client.cancel().await.unwrap();
}

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
        .call_tool(CallToolRequestParams::new("list_announcements"))
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    let items = json
        .as_array()
        .expect("structured content should be an array");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["id"], 1541631);
    assert_eq!(
        items[0]["title"],
        "Walk-About Garage Sale - Labor Day Weekend!"
    );
    assert_eq!(items[0]["posted_at"], "2026-08-13T16:14:37");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_moved_announcements_page_is_an_error_not_an_empty_list() {
    let server = MockServer::start();
    mock_login(&server);
    server.mock(|when, then| {
        when.method(GET).path("/announcement/show-list");
        then.status(200)
            .body("<html><body>nothing here</body></html>");
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(CallToolRequestParams::new("list_announcements"))
        .await;

    assert!(
        result.is_err() || result.unwrap().is_error == Some(true),
        "a moved page must not read as 'no announcements'"
    );
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty());
    client.cancel().await.unwrap();
}

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
        .call_tool(
            CallToolRequestParams::new("get_announcement").with_arguments(
                serde_json::json!({ "id": 1541631 })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();

    assert_ne!(result.is_error, Some(true), "tool errored: {result:?}");
    let json = serde_json::to_value(&result.structured_content).unwrap();
    assert_eq!(json["id"], 1541631);
    assert_eq!(json["title"], "Walk-About Garage Sale - Labor Day Weekend!");
    assert_eq!(json["posted_by"], "HPMA PRC");
    assert_eq!(json["posted_at"], "2026-08-13T16:14:37");
    assert_eq!(json["expires_at"], "2026-10-01T23:59:59");
    let body_md = json["body"].as_str().expect("body should be a string");
    assert!(
        !body_md.contains("<div"),
        "body must be markdown, got: {body_md}"
    );
    let attachments = json["attachments"]
        .as_array()
        .expect("attachments should be an array");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["name"], "Garage Sale.pdf");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_announcement_for_an_unknown_id_is_not_found() {
    let server = MockServer::start();
    mock_login(&server);
    server.mock(|when, then| {
        when.method(GET).path("/announcement/detail/999");
        then.status(404);
    });
    let client = connect(&server, 137473).await;

    let result = client
        .call_tool(
            CallToolRequestParams::new("get_announcement").with_arguments(
                serde_json::json!({ "id": 999 })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await;

    assert!(
        result.is_err(),
        "an unknown id should surface as a not-found error, got {result:?}"
    );
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.is_empty());
    client.cancel().await.unwrap();
}

// ---------------------------------------------------------------------------------------
// Violations
// ---------------------------------------------------------------------------------------

const FIXTURES: &str = "../condo-api/tests/fixtures";
const ATTACHMENT_KEY: &str = "00000000-0000-0000-0000-000000000000";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap()
}

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().cloned().unwrap()
}

/// The list page the client reads once for the workspace id and the filter dropdowns.
fn mock_list_page(server: &MockServer) {
    let body = fixture("violations_list_page.html");
    server.mock(|when, then| {
        when.method(GET).path("/violationtracking/list-violations/");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(body);
    });
}

fn mock_detail(server: &MockServer, body: String) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/violationTracking/view-violation-details/4242");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(body);
    });
}

/// A search response with one row per `LastUpdated` value.
fn search_json(total: u64, last_updated: &[&str]) -> String {
    let rows: Vec<_> = last_updated
        .iter()
        .enumerate()
        .map(|(i, lu)| {
            serde_json::json!({
                "ViolationName": format!("Example {i}"),
                "ViolationType": "Article IV Land Use Restrictions",
                "Unit": "101",
                "Stage": "Warning",
                "DueDate": "10/22/2026",
                "LastUpdated": lu,
                "ObjectNumber": i,
                "ViolationId": 100000 + i as u64,
            })
        })
        .collect();
    serde_json::json!({ "total": total, "rows": rows }).to_string()
}

fn mcp_error(result: Result<rmcp::model::CallToolResult, rmcp::ServiceError>) -> rmcp::ErrorData {
    match result {
        Err(rmcp::ServiceError::McpError(e)) => e,
        other => panic!("expected a tool error, got {other:?}"),
    }
}

async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &'static str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let result = client
        .call_tool(CallToolRequestParams::new(name).with_arguments(args(arguments)))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "tool error: {result:?}");
    serde_json::to_value(&result.structured_content).unwrap()
}

#[tokio::test]
async fn violation_tools_are_advertised_with_descriptions() {
    let server = MockServer::start();
    let client = connect(&server, 137473).await;
    let tools = client.list_all_tools().await.unwrap();
    for name in [
        "list_violations",
        "get_violation",
        "get_violation_update",
        "download_violation_attachment",
    ] {
        let t = tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .unwrap_or_else(|| panic!("{name} is not advertised"));
        assert!(
            t.description.as_ref().is_some_and(|d| !d.is_empty()),
            "{name} needs a description"
        );
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_violations_defaults_to_open_and_reports_total() {
    let server = MockServer::start();
    mock_login(&server);
    mock_list_page(&server);
    let body = fixture("violations_search.json");
    let search = server.mock(|when, then| {
        when.method(GET)
            .path("/violationTracking/search-violation-new/")
            .query_param("SelectedViolationStatusID", "1")
            .query_param("limit", "50");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    let client = connect(&server, 137473).await;

    let json = call(&client, "list_violations", serde_json::json!({})).await;

    search.assert();
    assert_eq!(json["total"], 918);
    assert_eq!(json["returned"], 3);
    assert_eq!(json["scan_complete"], true);
    let first = &json["violations"][0];
    assert_eq!(first["id"], 107330);
    assert_eq!(first["name"], "Unscreened Kayaks");
    assert_eq!(first["stage"], "30 Day Warning");
    assert_eq!(first["due_date"], "2026-10-18");
    assert_eq!(first["last_updated"], "2026-10-17");
    assert_eq!(first["last_updated_by"], "admin");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_violations_resolves_a_stage_name_to_every_matching_id() {
    let server = MockServer::start();
    mock_login(&server);
    mock_list_page(&server);
    let search = server.mock(|when, then| {
        when.method(GET)
            .path("/violationTracking/search-violation-new/")
            .query_param("SelectedStageIDs", "8418,9497")
            .query_param("SelectedViolationTypeIDs", "12241");
        then.status(200).body(search_json(0, &[]));
    });
    let client = connect(&server, 137473).await;

    let json = call(
        &client,
        "list_violations",
        serde_json::json!({ "stage": "warning", "violation_type": "article xiii moorage/marina" }),
    )
    .await;

    search.assert();
    assert_eq!(json["returned"], 0);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_violations_unknown_stage_lists_valid_names() {
    let server = MockServer::start();
    mock_login(&server);
    mock_list_page(&server);
    let client = connect(&server, 137473).await;

    let err = mcp_error(
        client
            .call_tool(
                CallToolRequestParams::new("list_violations")
                    .with_arguments(args(serde_json::json!({ "stage": "Nonexistent Stage" }))),
            )
            .await,
    );

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message
            .contains("Proposed Fine/Option for Appeal Hearing"),
        "message should list valid stage names: {}",
        err.message
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_violations_unknown_status_is_invalid_params() {
    let server = MockServer::start();
    let client = connect(&server, 137473).await;
    let err = mcp_error(
        client
            .call_tool(
                CallToolRequestParams::new("list_violations")
                    .with_arguments(args(serde_json::json!({ "status": "pending" }))),
            )
            .await,
    );
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("past_due"), "{}", err.message);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn updated_since_keeps_same_day_ties_and_stops_at_older_rows() {
    let server = MockServer::start();
    mock_login(&server);
    mock_list_page(&server);
    let search = server.mock(|when, then| {
        when.method(GET)
            .path("/violationTracking/search-violation-new/")
            .query_param("sort", "LastUpdated")
            .query_param("order", "desc");
        then.status(200).body(search_json(
            3,
            &["10/17/2026 admin", "10/17/2026", "10/16/2026 admin"],
        ));
    });
    let client = connect(&server, 137473).await;

    let json = call(
        &client,
        "list_violations",
        serde_json::json!({ "updated_since": "2026-10-17" }),
    )
    .await;

    search.assert_hits(1);
    assert_eq!(json["returned"], 2);
    assert_eq!(json["violations"].as_array().unwrap().len(), 2);
    assert_eq!(json["scan_complete"], true);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn updated_since_reports_incomplete_when_the_scan_cap_is_hit() {
    let server = MockServer::start();
    mock_login(&server);
    mock_list_page(&server);
    let rows = vec!["10/17/2026 admin"; 50];
    let search = server.mock(|when, then| {
        when.method(GET)
            .path("/violationTracking/search-violation-new/")
            .query_param("limit", "50");
        then.status(200).body(search_json(9999, &rows));
    });
    let client = connect(&server, 137473).await;

    let json = call(
        &client,
        "list_violations",
        serde_json::json!({ "updated_since": "2026-10-01", "limit": 200 }),
    )
    .await;

    search.assert_hits(10);
    assert_eq!(json["scan_complete"], false);
    assert_eq!(json["returned"], 200);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn updated_since_rejects_a_malformed_date() {
    let server = MockServer::start();
    let client = connect(&server, 137473).await;
    let err = mcp_error(
        client
            .call_tool(
                CallToolRequestParams::new("list_violations")
                    .with_arguments(args(serde_json::json!({ "updated_since": "10/17/2026" }))),
            )
            .await,
    );
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_returns_a_capped_newest_first_timeline() {
    let server = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    let client = connect(&server, 137473).await;

    let json = call(&client, "get_violation", serde_json::json!({ "id": 4242 })).await;

    assert_eq!(json["id"], 4242);
    assert_eq!(json["subject"], "Unscreened Boat Trailer Example");
    assert_eq!(json["update_count"], 3);
    assert_eq!(json["timeline_shown"], 3);
    let timeline = json["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 3);
    assert_eq!(timeline[0]["update"], "3", "newest first");
    assert_eq!(timeline[0]["posted_at"], "2026-03-20T11:20:00");
    assert_eq!(timeline[2]["update"], "1");
    for t in timeline {
        let s = t["snippet"].as_str().unwrap();
        assert!(s.len() <= 160 + 1, "snippet too long: {s}");
        assert!(!s.contains("  ") && !s.contains('\n'), "{s:?}");
    }
    assert!(timeline[0]["snippet"]
        .as_str()
        .unwrap()
        .starts_with("Dear Property Owner,"));
    let text = json.to_string();
    assert!(
        !text.contains(ATTACHMENT_KEY) && !text.contains("Key="),
        "{text}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_caps_the_timeline_at_25() {
    let server = MockServer::start();
    mock_login(&server);
    // Repeat the newest update table so the page carries 30 updates.
    let page = fixture("violation_detail.html");
    let start = page.find("<table border=\"0\"").unwrap();
    let end = start + page[start..].find("</table>").unwrap() + "</table>".len();
    let table = &page[start..end];
    let many = format!("{}{}{}", &page[..start], table.repeat(27), &page[start..]);
    mock_detail(&server, many);
    let client = connect(&server, 137473).await;

    let json = call(&client, "get_violation", serde_json::json!({ "id": 4242 })).await;

    assert_eq!(json["update_count"], 30);
    assert_eq!(json["timeline_shown"], 25);
    let timeline = json["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 25);
    assert_eq!(timeline[0]["update"], "30");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_adds_the_not_recorded_note_on_fine_track_stages_only() {
    let server = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    let client = connect(&server, 137473).await;
    let json = call(&client, "get_violation", serde_json::json!({ "id": 4242 })).await;
    assert_eq!(json["stage"], "Proposed Fine/Option for Appeal Hearing");
    let note = json["note"]
        .as_str()
        .expect("fine-track stage needs a note");
    assert!(note.contains("not recorded"), "{note}");
    client.cancel().await.unwrap();

    let server = MockServer::start();
    mock_login(&server);
    let warning = fixture("violation_detail.html").replace(
        ">Proposed Fine/Option for Appeal Hearing</span>",
        ">Warning</span>",
    );
    mock_detail(&server, warning);
    let client = connect(&server, 137473).await;
    let json = call(&client, "get_violation", serde_json::json!({ "id": 4242 })).await;
    assert_eq!(json["stage"], "Warning");
    assert!(json["note"].is_null(), "no note off the fine track: {json}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_never_labels_milestones() {
    for page in ["violation_detail.html", "violation_detail_empty.html"] {
        let server = MockServer::start();
        mock_login(&server);
        mock_detail(&server, fixture(page));
        let client = connect(&server, 137473).await;
        let json = call(&client, "get_violation", serde_json::json!({ "id": 4242 })).await;
        let text = json.to_string().to_lowercase();
        assert!(!text.contains("first notice"), "{text}");
        assert!(!text.contains("milestone"), "{text}");
        client.cancel().await.unwrap();
    }
    // Nor may the tool descriptions suggest it.
    let server = MockServer::start();
    let client = connect(&server, 137473).await;
    for t in client.list_all_tools().await.unwrap() {
        if t.name.contains("violation") {
            let d = t.description.unwrap_or_default().to_lowercase();
            assert!(
                !d.contains("first notice") && !d.contains("milestone"),
                "{d}"
            );
        }
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_update_returns_full_markdown_and_attachment_ids() {
    let server = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    let client = connect(&server, 137473).await;

    let json = call(
        &client,
        "get_violation_update",
        serde_json::json!({ "id": 4242, "update": "1", "expected_update_count": 3 }),
    )
    .await;

    assert_eq!(json["update"], "1");
    assert_eq!(json["posted_at"], "2026-03-04T08:05:00");
    assert_eq!(json["author"], "Compliance, Fake (202)");
    let body = json["body_markdown"].as_str().unwrap();
    assert!(body.contains("**Example Violation Heading:**"), "{body}");
    assert!(
        body.contains("Sincerely,"),
        "full body, not a snippet: {body}"
    );
    let attachments = json["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["attachment_id"], 1234567);
    let text = json.to_string();
    assert!(
        !text.contains(ATTACHMENT_KEY) && !text.contains("Key="),
        "{text}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_update_rejects_a_stale_update_count() {
    let server = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    let client = connect(&server, 137473).await;

    let err = mcp_error(
        client
            .call_tool(
                CallToolRequestParams::new("get_violation_update").with_arguments(args(
                    serde_json::json!({ "id": 4242, "update": "1", "expected_update_count": 2 }),
                )),
            )
            .await,
    );

    assert!(err.message.contains("get_violation"), "{}", err.message);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_violation_update_unknown_update_is_not_found() {
    let server = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    let client = connect(&server, 137473).await;

    for update in ["4", "0", "latest"] {
        let err = mcp_error(
            client
                .call_tool(
                    CallToolRequestParams::new("get_violation_update")
                        .with_arguments(args(serde_json::json!({ "id": 4242, "update": update }))),
                )
                .await,
        );
        assert_eq!(
            err.code,
            rmcp::model::ErrorCode::RESOURCE_NOT_FOUND,
            "{update}"
        );
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn download_violation_attachment_saves_under_the_configured_dir_and_returns_the_path() {
    let server = MockServer::start();
    let storage = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    let signed = format!(
        "{}?X-Amz-Expires=3600&X-Amz-Signature=SECRETSIG",
        storage.url("/2026/07/15/1234567.jpeg")
    );
    let access = server.mock(|when, then| {
        when.method(GET)
            .path("/access-file.aspx")
            .query_param("FileRecordID", "1234567")
            .query_param("Key", ATTACHMENT_KEY);
        then.status(302).header("location", signed.as_str());
    });
    let file = storage.mock(|when, then| {
        when.method(GET).path("/2026/07/15/1234567.jpeg");
        then.status(200)
            .header("content-type", "image/jpeg")
            .body("JPEGBYTES-SENTINEL");
    });
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("downloads");
    let client = connect_with_download_dir(&server, dest.clone()).await;

    let json = call(
        &client,
        "download_violation_attachment",
        serde_json::json!({ "violation_id": 4242, "attachment_id": 1234567 }),
    )
    .await;

    access.assert();
    file.assert();
    let path = std::path::PathBuf::from(json["path"].as_str().unwrap());
    assert!(path.starts_with(dest.canonicalize().unwrap()), "{path:?}");
    assert_eq!(std::fs::read(&path).unwrap(), b"JPEGBYTES-SENTINEL");
    assert_eq!(json["bytes"], 18);
    assert_eq!(json["content_type"], "image/jpeg");
    let text = json.to_string();
    assert!(!text.contains("JPEGBYTES"), "file contents leaked: {text}");
    assert!(
        !text.contains(ATTACHMENT_KEY) && !text.contains("Key"),
        "{text}"
    );
    assert!(
        !text.contains("X-Amz") && !text.contains("SECRETSIG"),
        "{text}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn download_errors_never_carry_the_signed_url_or_key() {
    let server = MockServer::start();
    mock_login(&server);
    mock_detail(&server, fixture("violation_detail.html"));
    // A redirect to a host outside the allowed family is refused.
    server.mock(|when, then| {
        when.method(GET).path("/access-file.aspx");
        then.status(302).header(
            "location",
            "https://evil.example.com/x.jpeg?X-Amz-Signature=SECRETSIG",
        );
    });
    let tmp = tempfile::tempdir().unwrap();
    let client = connect_with_download_dir(&server, tmp.path().to_path_buf()).await;

    let err = mcp_error(
        client
            .call_tool(
                CallToolRequestParams::new("download_violation_attachment").with_arguments(args(
                    serde_json::json!({ "violation_id": 4242, "attachment_id": 1234567 }),
                )),
            )
            .await,
    );
    let text = format!("{err:?}");
    assert!(
        !text.contains("SECRETSIG") && !text.contains("X-Amz"),
        "{text}"
    );
    assert!(
        !text.contains(ATTACHMENT_KEY) && !text.contains("evil"),
        "{text}"
    );
    assert!(err.message.to_lowercase().contains("attachment"), "{text}");
    client.cancel().await.unwrap();
}
