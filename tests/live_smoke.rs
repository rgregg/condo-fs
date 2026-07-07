// Opt-in live test. Run with:
//   CONDO_LIVE=1 CONDO_CREDS=~/tokens/condo-control.txt cargo test --test live_smoke -- --nocapture
use condo_fs::client::{CondoClient, HttpCondoClient};
use condo_fs::credentials::Credentials;
use condo_fs::model::Entry;
use std::path::PathBuf;

fn creds_path() -> PathBuf {
    let p = std::env::var("CONDO_CREDS").unwrap_or_else(|_| "~/tokens/condo-control.txt".into());
    if let Some(rest) = p.strip_prefix("~/") {
        return dirs::home_dir().unwrap().join(rest);
    }
    PathBuf::from(p)
}

#[test]
fn live_login_and_list_root() {
    if std::env::var("CONDO_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live test (set CONDO_LIVE=1 to run)");
        return;
    }
    let creds = Credentials::from_file(&creds_path()).unwrap();
    let client = HttpCondoClient::new("https://app.condocontrol.com", creds).unwrap();
    client.login().expect("login should succeed");
    let entries = client
        .list_folder(137473)
        .expect("root listing should succeed");
    assert!(!entries.is_empty(), "root folder should contain entries");
    let has_folder = entries.iter().any(|e| matches!(e, Entry::Folder { .. }));
    assert!(has_folder, "root should contain at least one folder");
    eprintln!("root has {} entries", entries.len());
}
