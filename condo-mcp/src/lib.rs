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
