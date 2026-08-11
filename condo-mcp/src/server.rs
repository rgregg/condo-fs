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
