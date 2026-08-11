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
    // The outbound query is widened by a day on each side of the requested
    // [start, end] range: the feed's own timestamps are naive association-local time
    // with no zone, so a query built directly from UTC-midnight Unix seconds could miss
    // in-range bookings depending on the association's UTC offset. 2025-08-01 widens to
    // 2025-07-31T00:00:00Z (1753920000) and 2025-08-31 widens to 2025-09-01T00:00:00Z
    // (1756684800); the tool then filters the parsed events back down to the exact
    // requested days (see list_events_includes_the_full_end_day_and_excludes_outside_the_range).
    let server = MockServer::start();
    mock_login(&server);
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/amenity/json-source/")
            .query_param("start", "1753920000")
            .query_param("end", "1756684800");
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
    // The endpoint's feed carries naive association-local timestamps with no zone, so
    // the tool widens its outbound query by a day on each side (proven by
    // list_events_converts_dates_to_unix_seconds) and then filters the parsed results by
    // comparing each event's naive date against the requested [start, end] range,
    // inclusive on both ends. This proves the filter half of that mechanism: an event on
    // the evening of the *end* date must survive (it used to be silently dropped by a
    // UTC-midnight cutoff), while events that only fall inside the widened fetch window —
    // one day before start, one day after end — must not leak into the result.
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
            "id": 3, "title": "Just after the requested range", "start": "2025-09-01T02:00:00",
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
