use clap::Parser;
use condo_fs::cache::ContentCache;
use condo_fs::client::{CondoClient, HttpCondoClient};
use condo_fs::config::{Cli, Command, MountArgs};
use condo_fs::credentials::Credentials;
use condo_fs::fs::CondoFs;
use condo_fs::vfs::Vfs;
use fuser::MountOption;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    match cli.command {
        Command::Mount(args) => {
            if let Err(e) = run_mount(args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn run_mount(args: MountArgs) -> Result<(), Box<dyn std::error::Error>> {
    let creds = Credentials::from_file(&args.credentials_path())?;
    let client = HttpCondoClient::new("https://app.condocontrol.com", creds)?;
    log::info!("authenticating…");
    client.login()?;
    log::info!(
        "authenticated; mounting folder {} at {}",
        args.root,
        args.mountpoint.display()
    );

    let cache = ContentCache::new(args.cache_dir_path())?;
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let vfs = Vfs::new(client, args.root, cache, args.meta_ttl_dur(), uid, gid);
    let fs = CondoFs::new(vfs);

    let options = vec![
        MountOption::RO,
        MountOption::FSName("condo".to_string()),
        MountOption::Subtype("condofuse".to_string()),
        MountOption::DefaultPermissions,
    ];
    fuser::mount2(fs, &args.mountpoint, &options)?;
    Ok(())
}
