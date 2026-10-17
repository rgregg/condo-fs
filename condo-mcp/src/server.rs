use crate::Args;
use condo_api::paths::expand_tilde;
use condo_api::{ClientError, CondoClient, Credentials, HttpCondoClient};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::ErrorData;
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
    /// Build a `CondoMcp`, constructing the underlying blocking `reqwest` client.
    ///
    /// # Panics
    ///
    /// `HttpCondoClient::new` builds a blocking `reqwest` client, which panics if it is
    /// constructed from inside a running Tokio runtime (a nested-runtime check). Do not call
    /// this directly from an `async fn` or `#[tokio::main]` body. Instead, run it inside
    /// `tokio::task::spawn_blocking`, as every call site in this crate does — see
    /// `main.rs` and the `test_client` helper in this module's tests.
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
            tool_router: Self::build_router(),
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
                // A poisoned lock carries no unsound state here — just a `bool` — so
                // recover it rather than propagating the poison. Worst case a login is
                // retried (harmless) or, if `*done` was already `true`, skipped as normal.
                let mut done = logged_in.lock().unwrap_or_else(|e| e.into_inner());
                if !*done {
                    client.login()?;
                    *done = true;
                }
            }
            f(&client)
        })
        .await
        // A JoinError here (task panic, or cancellation on shutdown) must not crash the
        // server or the tool-handler future — map it into the same error channel every
        // other client failure flows through so it reaches the agent as a tool error.
        .unwrap_or_else(|e| Err(ClientError::Io(std::io::Error::other(e))))
    }

    /// Turn a client error into a message an agent can act on.
    pub(crate) fn tool_error(&self, e: ClientError) -> ErrorData {
        // `NotFound` is a normal result, not a crash (see the error-handling table in
        // docs/superpowers/specs/2026-08-10-condo-mcp-design.md): the caller asked about
        // something that doesn't exist, not something the server failed to do. Map it to
        // `resource_not_found` rather than `internal_error` so it reads as an
        // invalid-parameter/not-found condition instead of a protocol-level failure.
        if let ClientError::NotFound = e {
            return ErrorData::resource_not_found("No such item in Condo Control.", None);
        }
        let msg = match e {
            ClientError::Auth => format!(
                "Authentication failed. Check the credentials file at {}.",
                self.creds_path
            ),
            ClientError::NotFound => unreachable!("returned above"),
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
        Self::library_router() + Self::calendar_router()
    }
}

#[rmcp::tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for CondoMcp {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_instructions(
            "Read-only access to a Condo Control condominium association. All tools are \
                 queries; nothing here can post, submit, or change anything.",
        )
        .with_server_info(rmcp::model::Implementation::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `CondoMcp` with `logged_in` pre-set to `true`, so `with_client` never
    /// attempts a real login — these tests are about the panic/poison handling around
    /// the lock and the `spawn_blocking` join, not about the client itself.
    async fn test_client(logged_in: bool) -> CondoMcp {
        // See the note in main.rs / tests/server.rs: constructing the blocking reqwest
        // client directly on this async test's worker thread would nest Tokio runtimes
        // and panic in debug builds, so build it via spawn_blocking.
        tokio::task::spawn_blocking(move || {
            let creds = Credentials {
                username: "u".into(),
                password: "p".into(),
            };
            CondoMcp {
                client: Arc::new(HttpCondoClient::new("http://127.0.0.1:1", creds).unwrap()),
                logged_in: Arc::new(Mutex::new(logged_in)),
                root: 1,
                creds_path: "<test>".into(),
                tool_router: CondoMcp::build_router(),
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn with_client_recovers_from_a_poisoned_login_lock() {
        let mcp = test_client(true).await;

        // Poison the lock the way a panicking `login()` would, without touching the
        // network: lock it and panic while holding the guard.
        let logged_in = mcp.logged_in.clone();
        let poisoning = std::thread::spawn(move || {
            let _guard = logged_in.lock().unwrap();
            panic!("simulated panic while holding the login lock");
        })
        .join();
        assert!(poisoning.is_err(), "the setup thread should have panicked");
        assert!(
            mcp.logged_in.is_poisoned(),
            "the lock should now be poisoned"
        );

        // A previously-broken with_client would propagate this poison as a panic here.
        let result = mcp.with_client(|_c| Ok(42)).await;
        assert_eq!(
            result.unwrap(),
            42,
            "recovers the lock instead of panicking"
        );
    }

    #[tokio::test]
    async fn a_panic_inside_with_client_becomes_a_client_error_not_a_crash() {
        let mcp = test_client(true).await;

        // The `f` closure panicking inside spawn_blocking is exactly the JoinError path
        // `with_client` must map instead of re-panicking on the async side.
        let result = mcp
            .with_client(|_c| -> Result<(), ClientError> { panic!("boom") })
            .await;
        assert!(
            matches!(result, Err(ClientError::Io(_))),
            "a panicked blocking task should surface as ClientError::Io, got {result:?}"
        );

        // The server (this same async context) must still be usable afterward.
        let ok = mcp.with_client(|_c| Ok(7)).await;
        assert_eq!(ok.unwrap(), 7);
    }
}
