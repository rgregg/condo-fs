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
    //
    // reqwest's blocking client builds its own internal Tokio runtime, which panics if
    // constructed directly on a worker thread of an existing runtime (the one `#[tokio::main]`
    // has already started by the time this line runs). Building it via `spawn_blocking`
    // gives it a plain OS thread instead, matching how every later client call is bridged.
    let server = tokio::task::spawn_blocking(move || CondoMcp::from_args(&args))
        .await
        .expect("blocking construction task panicked")?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
