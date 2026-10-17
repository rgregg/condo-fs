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

/// A small, fully deterministic (FNV-1a) hash — not std's `DefaultHasher`, whose docs
/// explicitly disclaim cross-compilation stability. We want the same input to always
/// fold to the same filename, on any machine, forever.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Fold a URL path (plus the `--xhr` flag, which changes the response for the same path)
/// into a filename that is safe, readable, and confined to one directory.
///
/// Two inputs that only differ past the 120-character truncation point, or the same path
/// probed with and without `--xhr`, would otherwise fold to the same readable prefix and
/// silently clobber each other's dump. A short hash of the *untruncated* `(path, xhr)`
/// pair is appended so distinct inputs always get distinct filenames.
fn safe_filename(path: &str, xhr: bool) -> String {
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

    let mut hash_input = path.as_bytes().to_vec();
    hash_input.push(if xhr { 1 } else { 0 });
    let hash = fnv1a(&hash_input);

    name.push('-');
    name.push_str(&format!("{:08x}", hash as u32));
    name
}

/// Write one raw response dump to `out_path`. Isolated in its own function so the caller
/// can treat any failure (disk full, permissions, ...) as a per-path warning rather than
/// aborting the whole run.
fn dump_response(out_path: &std::path::Path, raw: &condo_api::RawResponse) -> std::io::Result<()> {
    let mut f = std::fs::File::create(out_path)?;
    writeln!(f, "GET {}", raw.final_url)?;
    writeln!(f, "status: {}", raw.status)?;
    for (k, v) in &raw.headers {
        writeln!(f, "{k}: {v}")?;
    }
    writeln!(f)?;
    write!(f, "{}", raw.body)?;
    Ok(())
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
        let out_path = args
            .out_dir
            .join(format!("{}.txt", safe_filename(path, args.xhr)));
        if out_path.exists() {
            log::warn!(
                "  {} already exists and will be overwritten",
                out_path.display()
            );
        }
        // A write failure here is a local disk problem, not a reason to discard every
        // remaining path in the list: warn and move on, the same way an HTTP failure does
        // above, so one bad path doesn't waste a whole probing session against production.
        if let Err(e) = dump_response(&out_path, &raw) {
            log::warn!("{path}: failed to write {}: {e}", out_path.display());
            continue;
        }
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
    use super::{fnv1a, safe_filename};

    /// The exact hash suffix `safe_filename` appends, recomputed independently here so
    /// these tests verify the real algorithm rather than asserting against a copy-pasted
    /// magic string.
    fn expected_hash_suffix(path: &str, xhr: bool) -> String {
        let mut hash_input = path.as_bytes().to_vec();
        hash_input.push(if xhr { 1 } else { 0 });
        format!("{:08x}", fnv1a(&hash_input) as u32)
    }

    #[test]
    fn folds_path_separators_and_query_punctuation() {
        let path = "/library/get-file-list?mode=0&folderID=137473";
        assert_eq!(
            safe_filename(path, false),
            format!(
                "library_get-file-list_mode-0_folderID-137473-{}",
                expected_hash_suffix(path, false)
            )
        );
    }

    #[test]
    fn strips_leading_and_trailing_separators() {
        let path = "/announcements/";
        assert_eq!(
            safe_filename(path, false),
            format!("announcements-{}", expected_hash_suffix(path, false))
        );
    }

    #[test]
    fn truncates_very_long_paths() {
        let long = format!("/x/{}", "a".repeat(300));
        let name = safe_filename(&long, false);
        // The readable prefix (everything before the appended hash suffix) is capped at
        // 120 chars; the suffix itself is fixed-width and exempt from that cap.
        let readable = name.rsplit_once('-').map(|(prefix, _)| prefix).unwrap();
        assert!(readable.len() <= 120, "got {} chars", readable.len());
        assert!(name.starts_with("x_aaa"));
    }

    #[test]
    fn never_produces_a_path_traversal() {
        // A path from a list file must never escape the output directory.
        let name = safe_filename("/../../etc/passwd", false);
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }

    #[test]
    fn distinct_long_paths_sharing_a_120_char_prefix_get_distinct_filenames() {
        // Two paths that agree on their first 120+ significant characters and only
        // diverge after the truncation point used to fold to the identical filename,
        // silently clobbering each other's dump.
        let a = format!("/x/{}/first", "a".repeat(200));
        let b = format!("/x/{}/second", "a".repeat(200));
        let name_a = safe_filename(&a, false);
        let name_b = safe_filename(&b, false);
        assert_ne!(
            name_a, name_b,
            "distinct paths sharing a long common prefix must not collide"
        );
    }

    #[test]
    fn same_path_with_and_without_xhr_gets_distinct_filenames() {
        let path = "/announcements/get-announcement-list";
        let name_plain = safe_filename(path, false);
        let name_xhr = safe_filename(path, true);
        assert_ne!(
            name_plain, name_xhr,
            "the same path probed with and without --xhr must not collide"
        );
    }
}
