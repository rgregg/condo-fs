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
