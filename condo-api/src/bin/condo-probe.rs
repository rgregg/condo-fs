//! GET-only exploration tool for mapping undocumented Condo Control endpoints.
//!
//! Run with an explicit list of paths; nothing is crawled and no path is ever derived
//! from a response body. Every response is written verbatim to `--out-dir` for
//! inspection.
//!
//!   cargo run -p condo-api --features probe --bin condo-probe -- \
//!       --xhr /announcements/get-announcement-list /calendar/get-event-list
use clap::Parser;
use condo_api::paths::expand_tilde;
use condo_api::{CondoClient, Credentials, HttpCondoClient};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "condo-probe",
    about = "Fetch Condo Control paths (GET only) and dump the raw responses"
)]
struct Args {
    /// Paths to fetch, e.g. /announcements/get-announcement-list
    #[arg(required = true)]
    paths: Vec<String>,

    /// Credentials file (KEY=VALUE lines: USERNAME, PASSWORD)
    #[arg(long, default_value = "~/tokens/condo-control.txt")]
    credentials: String,

    #[arg(long, default_value = "https://app.condocontrol.com")]
    base_url: String,

    /// Where to write the response dumps
    #[arg(long, default_value = "probe-out")]
    out_dir: PathBuf,

    /// Send X-Requested-With: XMLHttpRequest (needed by the JSON endpoints)
    #[arg(long)]
    xhr: bool,

    /// Milliseconds to wait between requests
    #[arg(long, default_value_t = 1000)]
    delay_ms: u64,
}

/// Fold a URL path into a filename that is safe, readable, and confined to one directory.
fn safe_filename(path: &str) -> String {
    let mut out = String::new();
    for ch in path.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => out.push(ch),
            '=' => out.push('-'),
            _ => {
                if !out.ends_with('_') {
                    out.push('_');
                }
            }
        }
    }
    let trimmed = out.trim_matches('_');
    let mut name: String = trimmed.chars().take(120).collect();
    if name.is_empty() {
        name.push_str("root");
    }
    name
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let creds = Credentials::from_file(&expand_tilde(&args.credentials))?;
    let client = HttpCondoClient::new(&args.base_url, creds)?;
    log::info!("authenticating…");
    client.login()?;

    std::fs::create_dir_all(&args.out_dir)?;

    for (i, path) in args.paths.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(args.delay_ms));
        }
        log::info!("GET {path}");
        let raw = match client.get_raw(path, args.xhr) {
            Ok(raw) => raw,
            Err(e) => {
                log::warn!("{path}: {e}");
                continue;
            }
        };
        let out_path = args.out_dir.join(format!("{}.txt", safe_filename(path)));
        let mut f = std::fs::File::create(&out_path)?;
        writeln!(f, "GET {}", raw.final_url)?;
        writeln!(f, "status: {}", raw.status)?;
        for (k, v) in &raw.headers {
            writeln!(f, "{k}: {v}")?;
        }
        writeln!(f)?;
        write!(f, "{}", raw.body)?;
        log::info!(
            "  {} → {} ({} bytes) → {}",
            raw.status,
            raw.headers
                .iter()
                .find(|(k, _)| k == "content-type")
                .map(|(_, v)| v.as_str())
                .unwrap_or("?"),
            raw.body.len(),
            out_path.display()
        );
        if raw.status == 302 {
            log::warn!("  redirect — session may not be authenticated for this area");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::safe_filename;

    #[test]
    fn folds_path_separators_and_query_punctuation() {
        assert_eq!(
            safe_filename("/library/get-file-list?mode=0&folderID=137473"),
            "library_get-file-list_mode-0_folderID-137473"
        );
    }

    #[test]
    fn strips_leading_and_trailing_separators() {
        assert_eq!(safe_filename("/announcements/"), "announcements");
    }

    #[test]
    fn truncates_very_long_paths() {
        let long = format!("/x/{}", "a".repeat(300));
        let name = safe_filename(&long);
        assert!(name.len() <= 120, "got {} chars", name.len());
        assert!(name.starts_with("x_aaa"));
    }

    #[test]
    fn never_produces_a_path_traversal() {
        // A path from a list file must never escape the output directory.
        let name = safe_filename("/../../etc/passwd");
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }
}
