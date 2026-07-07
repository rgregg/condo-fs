use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "condo-fs",
    about = "Mount a Condo Control File Library as a read-only filesystem"
)]
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
    pub fn credentials_path(&self) -> PathBuf {
        expand_tilde(&self.credentials)
    }
    pub fn cache_dir_path(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("condo-fs")
        })
    }
    pub fn meta_ttl_dur(&self) -> Duration {
        Duration::from_secs(self.meta_ttl)
    }
}
