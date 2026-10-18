use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{
    parse_violation_detail, parse_violation_filters, parse_violation_page, SavedAttachment,
    ViolationDetail, ViolationFilterOptions, ViolationPage, ViolationQuery,
};
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE, LOCATION};
use reqwest::StatusCode;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest sanitised name kept, in bytes.
const MAX_NAME_BYTES: usize = 150;

/// Error text for a redirect we refuse to follow. Fixed, so it can never echo the URL.
const UNEXPECTED_LOCATION: &str = "unexpected download location";
const TOO_LARGE: &str = "file exceeds size limit";

fn attachment_error(msg: &str) -> ClientError {
    ClientError::Attachment(msg.to_string())
}

/// True only for `amazonaws.com` and its subdomains: the family the site's signed
/// attachment redirects point at.
pub(crate) fn is_storage_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "amazonaws.com" || h.ends_with(".amazonaws.com")
}

/// Make `name` safe as a single path component: no separators, no control characters, no
/// `..`, no leading or trailing dots, bounded length, and never empty.
pub(crate) fn sanitise_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    let trim = |c: char| c == '.' || c == '_' || c.is_whitespace();
    let mut s = s.trim_matches(trim).to_string();
    if s.len() > MAX_NAME_BYTES {
        let mut cut = MAX_NAME_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s = s.trim_end_matches(trim).to_string();
    }
    if s.is_empty() {
        "attachment".to_string()
    } else {
        s
    }
}

/// The media type without parameters, lower-cased.
fn media_type(header: &str) -> Option<String> {
    let essence = header.split(';').next()?.trim().to_ascii_lowercase();
    (!essence.is_empty()).then_some(essence)
}

/// For a media type: the extension to append, followed by every extension that already
/// matches it. Unlisted types get no extension.
fn type_extensions(media_type: &str) -> Option<&'static [&'static str]> {
    Some(match media_type {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => &["jpg", "jpeg", "jpe"],
        "image/png" => &["png"],
        "image/gif" => &["gif"],
        "image/webp" => &["webp"],
        "image/heic" | "image/heif" => &["heic", "heif"],
        "image/tiff" => &["tiff", "tif"],
        "application/pdf" => &["pdf"],
        "audio/mpeg" | "audio/mp3" => &["mp3"],
        "audio/mp4" | "audio/x-m4a" => &["m4a"],
        "video/mp4" => &["mp4", "m4v"],
        "video/quicktime" => &["mov", "qt"],
        "text/plain" => &["txt", "text", "log"],
        "application/zip" | "application/x-zip-compressed" => &["zip"],
        "application/msword" => &["doc"],
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => &["docx"],
        "application/vnd.ms-excel" => &["xls"],
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => &["xlsx"],
        _ => return None,
    })
}

/// Append the media type's extension to an already-sanitised `name` unless it already ends
/// in one consistent with that type. The site's attachment names are display labels
/// ("img 8526", "Mr. Smith photo"), so a trailing dot-segment counts as an extension only
/// when it is 1–5 ASCII alphanumerics; with an unknown type the name is left alone.
fn with_type_extension(name: String, media_type: Option<&str>) -> String {
    let Some(exts) = media_type.and_then(type_extensions) else {
        return name;
    };
    let existing = name
        .rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| (1..=5).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_alphanumeric()));
    if existing.is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e))) {
        return name;
    }
    format!("{name}.{}", exts[0])
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(hi), Some(lo)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The filename a `Content-Disposition` header suggests; `filename*` (RFC 6266/5987) wins
/// over `filename`.
fn content_disposition_filename(header: &str) -> Option<String> {
    let (mut plain, mut extended) = (None, None);
    for part in header.split(';') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                // charset'language'percent-encoded
                if let Some((_, encoded)) = v.split_once('\'').and_then(|(_, r)| r.split_once('\''))
                {
                    extended = Some(percent_decode(encoded.trim_matches('"')));
                }
            }
            "filename" => plain = Some(v.trim_matches('"').to_string()),
            _ => {}
        }
    }
    extended.or(plain).filter(|n| !n.trim().is_empty())
}

/// Copy `r` into `w`, failing before any byte past `max` is written.
fn copy_capped(r: &mut dyn Read, w: &mut dyn Write, max: u64) -> Result<u64, ClientError> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            // A body read error can wrap a reqwest error that names the signed URL, so it is
            // never surfaced; only its kind is logged.
            Err(e) => {
                log::warn!("attachment download interrupted ({:?})", e.kind());
                return Err(attachment_error("download was interrupted"));
            }
        };
        total += n as u64;
        if total > max {
            return Err(attachment_error(TOO_LARGE));
        }
        w.write_all(&buf[..n])?;
    }
}

/// Stream `r` into `dir/file_name` through a hidden partial file that is renamed into place
/// only once complete, and removed on any failure.
fn stream_to_file(
    r: &mut dyn Read,
    dir: &Path,
    file_name: &str,
    max: u64,
) -> Result<u64, ClientError> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let partial = dir.join(format!(
        ".{file_name}.{}-{}.part",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        // `create_new` refuses to write through anything already at that path.
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        let n = copy_capped(r, &mut f, max)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&partial, dir.join(file_name))?;
        Ok(n)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

impl HttpCondoClient {
    /// The list page's workspace id and dropdown options, fetched once and then cached.
    ///
    /// Only a successful parse is cached, so a failure (expired session, moved markup) is
    /// retried on the next call. The lock is held across the fetch so concurrent callers
    /// do not each hit the page.
    pub(crate) fn violation_filters_cached(&self) -> Result<ViolationFilterOptions, ClientError> {
        let mut guard = self
            .violation_filters
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(f) = guard.as_ref() {
            return Ok(f.clone());
        }
        let f = self.with_reauth(|| self.violation_filters_once())?;
        *guard = Some(f.clone());
        Ok(f)
    }

    fn violation_filters_once(&self) -> Result<ViolationFilterOptions, ClientError> {
        let resp = self
            .http
            .get(self.url("/violationtracking/list-violations/"))
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let html = resp.error_for_status()?.text()?;
        Ok(parse_violation_filters(&html)?)
    }

    /// One attempt at a page of the violations list (a JSON XHR endpoint).
    pub(crate) fn list_violations_once(
        &self,
        q: &ViolationQuery,
    ) -> Result<ViolationPage, ClientError> {
        let workspace_id = self.violation_filters_cached()?.workspace_id;
        let join = |ids: &[u64]| ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let params = [
            ("WorkspaceID", workspace_id.to_string()),
            ("SearchTerm", q.search.clone().unwrap_or_default()),
            ("SelectedViolationStatusID", q.status.site_id().to_string()),
            ("SelectedViolationTypeIDs", join(&q.type_ids)),
            ("SelectedCreatorID", "-1".to_string()),
            ("SelectedStageIDs", join(&q.stage_ids)),
            ("userIsJustVoter", "False".to_string()),
            ("limit", q.limit.to_string()),
            ("offset", q.offset.to_string()),
            ("sort", "LastUpdated".to_string()),
            ("order", "desc".to_string()),
        ];
        let resp = self
            .http
            .get(self.url("/violationTracking/search-violation-new/"))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(&params)
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let body = resp.error_for_status()?.text()?;
        parse_violation_page(&body)
    }

    /// One attempt at a single violation's detail page.
    ///
    /// `status=-1` ("all") matches what the site's own list links send. The page returns the
    /// same updates without it, but we send it so the request mirrors the site.
    pub(crate) fn get_violation_once(&self, id: u64) -> Result<ViolationDetail, ClientError> {
        let resp = self
            .http
            .get(self.url(&format!("/violationTracking/view-violation-details/{id}")))
            .query(&[("status", "-1")])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ClientError::NotFound);
        }
        let html = resp.error_for_status()?.text()?;
        Ok(parse_violation_detail(id, &html)?)
    }

    /// One attempt at downloading an attachment. See `ViolationsApi::download_violation_attachment`.
    ///
    /// Nothing here may put the attachment `Key`, the `access-file.aspx` URL or the signed
    /// storage URL into a log line or an error: reqwest errors from either request are
    /// stripped of their URL or replaced by a fixed message.
    pub(crate) fn download_violation_attachment_once(
        &self,
        violation_id: u64,
        attachment_id: u64,
        dest_dir: &Path,
        max_bytes: u64,
    ) -> Result<SavedAttachment, ClientError> {
        // Stateless: the key is read fresh from the page every time, never cached.
        let detail = self.get_violation_once(violation_id)?;
        let attachment = detail
            .updates
            .iter()
            .flat_map(|u| &u.attachments)
            .find(|a| a.id == attachment_id)
            .ok_or(ClientError::NotFound)?;

        // Step 1: the authenticated request. This client never follows redirects.
        log::info!("requesting attachment FileRecordID={attachment_id}");
        let resp = self
            .http
            .get(self.url("/access-file.aspx"))
            .query(&[
                ("FileRecordID", attachment_id.to_string()),
                ("Key", attachment.key.clone()),
                ("mode", "preview".to_string()),
            ])
            .send()
            .map_err(|e| ClientError::Http(e.without_url()))?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Err(ClientError::NotFound);
        }
        if !status.is_redirection() {
            log::warn!(
                "attachment FileRecordID={attachment_id}: site answered HTTP {} without a redirect",
                status.as_u16()
            );
            return Err(attachment_error("the site did not hand out the file"));
        }
        let target = resp
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|loc| resp.url().join(loc).ok())
            .ok_or_else(|| attachment_error(UNEXPECTED_LOCATION))?;
        if target.origin() == resp.url().origin()
            && target.path().to_ascii_lowercase().starts_with("/login")
        {
            return Err(ClientError::Auth);
        }
        drop(resp);
        let scheme_ok = target.scheme() == "https"
            || (!self.storage_require_https && target.scheme() == "http");
        let host_ok = target.host_str().is_some_and(|h| (self.storage_host_ok)(h));
        if !scheme_ok || !host_ok || !target.username().is_empty() || target.password().is_some() {
            log::warn!("attachment FileRecordID={attachment_id}: refused an unexpected redirect");
            return Err(attachment_error(UNEXPECTED_LOCATION));
        }

        // Step 2: the signed URL, on the cookie-less storage client.
        let mut resp = self.storage.get(target).send().map_err(|e| {
            log::warn!(
                "attachment FileRecordID={attachment_id}: storage request failed (connect: {}, timeout: {})",
                e.is_connect(),
                e.is_timeout()
            );
            attachment_error("could not reach the file storage")
        })?;
        if !resp.status().is_success() {
            log::warn!(
                "attachment FileRecordID={attachment_id}: storage answered HTTP {}",
                resp.status().as_u16()
            );
            return Err(attachment_error("file storage refused the download"));
        }
        if resp.content_length().is_some_and(|n| n > max_bytes) {
            return Err(attachment_error(TOO_LARGE));
        }
        let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok());
        let content_type = header(CONTENT_TYPE).and_then(media_type);
        let name = attachment
            .name
            .clone()
            .or_else(|| header(CONTENT_DISPOSITION).and_then(content_disposition_filename))
            .unwrap_or_default();
        // Sanitise first (an empty name becomes "attachment"), then add the extension, so
        // the appended part can never reintroduce a separator.
        let name = with_type_extension(sanitise_filename(&name), content_type.as_deref());
        let file_name = format!("violation-{violation_id}-{attachment_id}-{name}");

        fs::create_dir_all(dest_dir)?;
        let dir = dest_dir.canonicalize()?;
        let bytes = stream_to_file(&mut resp, &dir, &file_name, max_bytes)?;
        let path = dir.join(&file_name).canonicalize()?;
        if path.parent() != Some(dir.as_path()) {
            let _ = fs::remove_file(dir.join(&file_name));
            return Err(attachment_error(
                "refused to save outside the download folder",
            ));
        }
        log::info!("saved attachment FileRecordID={attachment_id}: {bytes} bytes");
        Ok(SavedAttachment {
            path,
            bytes,
            content_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{
        ClientError, HttpCondoClient, LoginApi, ViolationQuery, ViolationStatus, ViolationsApi,
    };
    use httpmock::prelude::*;

    fn query() -> ViolationQuery {
        ViolationQuery {
            status: ViolationStatus::All,
            search: None,
            type_ids: vec![],
            stage_ids: vec![],
            offset: 0,
            limit: 25,
        }
    }

    fn list_page_mock(server: &MockServer) -> httpmock::Mock<'_> {
        let html = std::fs::read_to_string("tests/fixtures/violations_list_page.html").unwrap();
        server.mock(|when, then| {
            when.method(GET).path("/violationtracking/list-violations/");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(html);
        })
    }

    fn json_body() -> String {
        std::fs::read_to_string("tests/fixtures/violations_search.json").unwrap()
    }

    #[test]
    fn list_violations_sends_the_site_query_and_xhr_header() {
        let server = MockServer::start();
        list_page_mock(&server);
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/search-violation-new/")
                .header("x-requested-with", "XMLHttpRequest")
                .query_param("WorkspaceID", "13582")
                .query_param("SelectedViolationStatusID", "-1")
                .query_param("limit", "25")
                .query_param("offset", "0")
                .query_param("sort", "LastUpdated")
                .query_param("order", "desc")
                .query_param("userIsJustVoter", "False");
            then.status(200)
                .header("content-type", "application/json")
                .body(json_body());
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let page = client.list_violations(&query()).unwrap();
        m.assert();
        assert_eq!(page.total, 918);
        assert_eq!(page.rows.len(), 3);
    }

    #[test]
    fn search_text_and_ids_are_sent() {
        let server = MockServer::start();
        list_page_mock(&server);
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/search-violation-new/")
                .query_param("SearchTerm", "kayak & co")
                .query_param("SelectedViolationTypeIDs", "3,4")
                .query_param("SelectedStageIDs", "8418,9497")
                .query_param("SelectedViolationStatusID", "1")
                .query_param("offset", "50");
            then.status(200).body(json_body());
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let q = ViolationQuery {
            status: ViolationStatus::Open,
            search: Some("kayak & co".into()),
            type_ids: vec![3, 4],
            stage_ids: vec![8418, 9497],
            offset: 50,
            limit: 25,
        };
        client.list_violations(&q).unwrap();
        m.assert();
    }

    #[test]
    fn type_and_stage_ids_are_comma_joined() {
        let server = MockServer::start();
        list_page_mock(&server);
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/search-violation-new/")
                .query_param("SelectedViolationTypeIDs", "3,4");
            then.status(200).body(json_body());
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let mut q = query();
        q.type_ids = vec![3, 4];
        client.list_violations(&q).unwrap();
        m.assert();
    }

    #[test]
    fn filters_are_fetched_once_and_cached() {
        let server = MockServer::start();
        let page = list_page_mock(&server);
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/search-violation-new/");
            then.status(200).body(json_body());
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        client.list_violations(&query()).unwrap();
        client.list_violations(&query()).unwrap();
        let f = client.violation_filters().unwrap();
        assert_eq!(f.workspace_id, 13582);
        page.assert_hits(1);
    }

    #[test]
    fn list_violations_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/violationtracking/list-violations/");
            then.status(302).header("location", "/login?NextPage=x");
        });
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.list_violations(&query()),
            Err(ClientError::Auth)
        ));
    }

    #[test]
    fn search_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        list_page_mock(&server);
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/search-violation-new/");
            then.status(302).header("location", "/login?NextPage=x");
        });
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.list_violations(&query()),
            Err(ClientError::Auth)
        ));
    }

    fn login_mocks(server: &MockServer) {
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
    }

    #[test]
    fn get_violation_fetches_by_id_with_status_all() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/violation_detail.html").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/view-violation-details/4242")
                .query_param("status", "-1");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let d = client.get_violation(4242).unwrap();
        m.assert();
        assert_eq!(d.subject, "Unscreened Boat Trailer Example");
        assert_eq!(d.updates.len(), 3);
    }

    #[test]
    fn get_violation_404_is_not_found() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/view-violation-details/999");
            then.status(404);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.get_violation(999),
            Err(ClientError::NotFound)
        ));
    }

    #[test]
    fn get_violation_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/view-violation-details/4242");
            then.status(302).header("location", "/login?NextPage=x");
        });
        login_mocks(&server);
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.get_violation(4242), Err(ClientError::Auth)));
    }

    #[test]
    fn get_violation_200_shell_is_a_structure_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/view-violation-details/4242");
            then.status(200)
                .body("<html><body>Invalid address</body></html>");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.get_violation(4242),
            Err(ClientError::PageStructure(_))
        ));
    }

    // ---- attachment download ----

    use super::{
        copy_capped, is_storage_host, sanitise_filename, stream_to_file, with_type_extension,
    };
    use std::path::Path;

    const KEY: &str = "00000000-0000-0000-0000-000000000000";
    const SIGNED_PATH: &str = "/2026/07/15/1234567.jpeg";
    const SIGNED_QUERY: &str =
        "X-Amz-Expires=3600&X-Amz-Security-Token=SECRETTOKEN&X-Amz-Signature=SECRETSIG";

    #[test]
    fn sanitise_filename_strips_separators_and_traversal() {
        for raw in ["../../etc/passwd", "/abs/x.pdf", "a\\b.jpg"] {
            let s = sanitise_filename(raw);
            assert!(!s.is_empty(), "{raw:?} -> empty");
            assert!(!s.contains('/'), "{raw:?} -> {s:?}");
            assert!(!s.contains('\\'), "{raw:?} -> {s:?}");
            assert!(!s.contains(".."), "{raw:?} -> {s:?}");
            assert!(!s.starts_with('.'), "{raw:?} -> {s:?}");
        }
    }

    #[test]
    fn sanitise_filename_never_returns_empty() {
        for raw in ["", "...", "///"] {
            assert_eq!(sanitise_filename(raw), "attachment", "{raw:?}");
        }
    }

    #[test]
    fn sanitise_filename_keeps_a_normal_name() {
        assert_eq!(sanitise_filename("Notice 1.pdf"), "Notice 1.pdf");
    }

    #[test]
    fn sanitise_filename_drops_control_characters_and_caps_length() {
        let s = sanitise_filename("a\u{0}b\nc.pdf");
        assert!(!s.chars().any(char::is_control), "{s:?}");
        assert!(sanitise_filename(&"x".repeat(1000)).len() <= 150);
    }

    #[test]
    fn is_storage_host_accepts_s3_only() {
        assert!(is_storage_host(
            "ccc-filedata-prod.s3.ca-central-1.amazonaws.com"
        ));
        for h in ["evil.com", "amazonaws.com.evil.com", "notamazonaws.com", ""] {
            assert!(!is_storage_host(h), "{h}");
        }
    }

    #[test]
    fn copy_capped_stops_before_writing_past_the_limit() {
        let mut out = Vec::new();
        let err = copy_capped(&mut std::io::Cursor::new(vec![1u8, 2, 3]), &mut out, 2).unwrap_err();
        assert!(matches!(err, ClientError::Attachment(ref m) if m == "file exceeds size limit"));
        assert!(
            out.len() <= 2,
            "wrote {} bytes past a 2-byte cap",
            out.len()
        );
        let mut out = Vec::new();
        let n = copy_capped(&mut std::io::Cursor::new(vec![1u8, 2, 3]), &mut out, 3).unwrap();
        assert_eq!((n, out), (3, vec![1, 2, 3]));
    }

    #[test]
    fn stream_to_file_removes_the_partial_file_when_over_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let err = stream_to_file(
            &mut std::io::Cursor::new(vec![7u8; 10]),
            dir.path(),
            "violation-1-2-x.bin",
            4,
        )
        .unwrap_err();
        assert!(matches!(err, ClientError::Attachment(_)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    fn detail_mock(server: &MockServer, alt: Option<&str>) {
        let mut body = std::fs::read_to_string("tests/fixtures/violation_detail.html").unwrap();
        if let Some(alt) = alt {
            body = body.replace(
                "<img src=\"https://app.condocontrol.com/access-file",
                &format!("<img alt=\"{alt}\" src=\"https://app.condocontrol.com/access-file"),
            );
        }
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/view-violation-details/4242");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(body);
        });
    }

    fn access_mock<'a>(server: &'a MockServer, location: &str) -> httpmock::Mock<'a> {
        server.mock(|when, then| {
            when.method(GET)
                .path("/access-file.aspx")
                .query_param("FileRecordID", "1234567")
                .query_param("Key", KEY)
                .query_param("mode", "preview");
            then.status(302).header("location", location);
        })
    }

    fn signed_url(storage: &MockServer) -> String {
        format!("{}?{SIGNED_QUERY}", storage.url(SIGNED_PATH))
    }

    fn local_client(server: &MockServer) -> HttpCondoClient {
        HttpCondoClient::new(server.base_url(), creds())
            .unwrap()
            .with_storage_host_check(|h| h == "127.0.0.1")
    }

    fn violation_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .filter(|n| n.contains("violation-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn assert_no_secrets(err: &ClientError) {
        for text in [err.to_string(), format!("{err:?}")] {
            assert!(!text.contains(KEY), "key leaked: {text}");
            assert!(!text.contains("X-Amz"), "signed query leaked: {text}");
            assert!(!text.contains("SECRET"), "signed query leaked: {text}");
            assert!(!text.contains(SIGNED_PATH), "signed path leaked: {text}");
            assert!(!text.contains("access-file"), "site URL leaked: {text}");
        }
    }

    #[test]
    fn downloads_through_the_redirect_and_saves_the_file() {
        let server = MockServer::start();
        let storage = MockServer::start();
        detail_mock(&server, None);
        let access = access_mock(&server, &signed_url(&storage));
        let file = storage.mock(|when, then| {
            when.method(GET)
                .path(SIGNED_PATH)
                .query_param("X-Amz-Signature", "SECRETSIG");
            then.status(200)
                .header("content-type", "image/jpeg")
                .body([1u8, 2, 3]);
        });
        // Not created up front: the download must create it.
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("downloads");
        let saved = local_client(&server)
            .download_violation_attachment(4242, 1234567, &dest, 1024)
            .unwrap();
        access.assert();
        file.assert();
        assert_eq!(std::fs::read(&saved.path).unwrap(), vec![1, 2, 3]);
        let name = saved
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("violation-4242-1234567-"), "{name}");
        assert!(saved.path.starts_with(dest.canonicalize().unwrap()));
        assert_eq!(saved.bytes, 3);
        assert_eq!(saved.content_type.as_deref(), Some("image/jpeg"));
        let dbg = format!("{saved:?}");
        assert!(!dbg.contains("X-Amz") && !dbg.contains(KEY), "{dbg}");
    }

    #[test]
    fn storage_request_carries_no_cookies_or_session_headers() {
        let server = MockServer::start();
        let storage = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200)
                .header("set-cookie", "ASP.NET_SessionId=abc; path=/");
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302)
                .header("location", "/my/my-home")
                .header("set-cookie", "CCCookie=xyz; path=/");
        });
        detail_mock(&server, None);
        // The main client really does hold a session: the access request carries it.
        let access = server.mock(|when, then| {
            when.method(GET)
                .path("/access-file.aspx")
                .header_exists("cookie");
            then.status(302).header("location", signed_url(&storage));
        });
        let clean = storage.mock(|when, then| {
            when.method(GET).path(SIGNED_PATH).matches(|req| {
                !req.headers.as_ref().is_some_and(|h| {
                    h.iter().any(|(k, _)| {
                        k.eq_ignore_ascii_case("cookie")
                            || k.eq_ignore_ascii_case("authorization")
                            || k.eq_ignore_ascii_case("referer")
                            || k.eq_ignore_ascii_case("x-requested-with")
                    })
                })
            });
            then.status(200)
                .header("content-type", "image/jpeg")
                .body([1u8, 2, 3]);
        });
        let client = local_client(&server);
        client.login().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        client
            .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
            .unwrap();
        access.assert();
        clean.assert();
    }

    #[test]
    fn redirect_to_an_unexpected_host_is_refused_and_nothing_is_fetched() {
        let server = MockServer::start();
        detail_mock(&server, None);
        access_mock(&server, &format!("https://evil.example/x?{SIGNED_QUERY}"));
        // Default host check: nothing but an amazonaws.com host may be followed.
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = client
            .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
            .unwrap_err();
        assert!(
            matches!(err, ClientError::Attachment(ref m) if m == "unexpected download location"),
            "{err:?}"
        );
        assert!(violation_files(tmp.path()).is_empty());
    }

    #[test]
    fn plain_http_storage_url_is_refused_by_default() {
        let server = MockServer::start();
        detail_mock(&server, None);
        access_mock(
            &server,
            &format!("http://bucket.s3.amazonaws.com/x?{SIGNED_QUERY}"),
        );
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = client
            .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
            .unwrap_err();
        assert!(
            matches!(err, ClientError::Attachment(ref m) if m == "unexpected download location"),
            "{err:?}"
        );
    }

    #[test]
    fn oversize_download_is_aborted_and_the_partial_file_removed() {
        let server = MockServer::start();
        let storage = MockServer::start();
        detail_mock(&server, None);
        access_mock(&server, &signed_url(&storage));
        storage.mock(|when, then| {
            when.method(GET).path(SIGNED_PATH);
            then.status(200)
                .header("content-type", "image/jpeg")
                .body([1u8, 2, 3]);
        });
        let tmp = tempfile::tempdir().unwrap();
        let err = local_client(&server)
            .download_violation_attachment(4242, 1234567, tmp.path(), 2)
            .unwrap_err();
        assert!(
            matches!(err, ClientError::Attachment(ref m) if m == "file exceeds size limit"),
            "{err:?}"
        );
        assert!(violation_files(tmp.path()).is_empty());
    }

    #[test]
    fn unknown_attachment_id_is_not_found() {
        let server = MockServer::start();
        detail_mock(&server, None);
        let access = server.mock(|when, then| {
            when.method(GET).path("/access-file.aspx");
            then.status(500);
        });
        let tmp = tempfile::tempdir().unwrap();
        let err = local_client(&server)
            .download_violation_attachment(4242, 999, tmp.path(), 1024)
            .unwrap_err();
        assert!(matches!(err, ClientError::NotFound), "{err:?}");
        access.assert_hits(0);
    }

    #[test]
    fn access_redirect_to_login_is_an_auth_error() {
        let server = MockServer::start();
        detail_mock(&server, None);
        access_mock(&server, "/login?NextPage=x");
        login_mocks(&server);
        let tmp = tempfile::tempdir().unwrap();
        let err = local_client(&server)
            .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
            .unwrap_err();
        assert!(matches!(err, ClientError::Auth), "{err:?}");
    }

    #[test]
    fn errors_never_contain_the_key_or_signed_url() {
        let tmp = tempfile::tempdir().unwrap();
        let mut errors = Vec::new();

        // Unexpected host.
        let server = MockServer::start();
        detail_mock(&server, None);
        access_mock(
            &server,
            &format!("https://evil.example{SIGNED_PATH}?{SIGNED_QUERY}"),
        );
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        errors.push(
            client
                .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
                .unwrap_err(),
        );

        // Oversize, storage refusal, and an unreachable storage host: a fresh pair of
        // servers per case.
        let storage_case = |status: u16, body: &[u8], max: u64, dead: bool| {
            let server = MockServer::start();
            let storage = MockServer::start();
            detail_mock(&server, None);
            let location = if dead {
                // Nothing listens on 127.0.0.1:9 (discard), so the storage GET fails to connect.
                format!("http://127.0.0.1:9{SIGNED_PATH}?{SIGNED_QUERY}")
            } else {
                signed_url(&storage)
            };
            access_mock(&server, &location);
            storage.mock(|when, then| {
                when.method(GET).path(SIGNED_PATH);
                then.status(status).body(body);
            });
            local_client(&server)
                .download_violation_attachment(4242, 1234567, tmp.path(), max)
                .unwrap_err()
        };
        errors.push(storage_case(200, &[1, 2, 3], 2, false));
        errors.push(storage_case(
            403,
            b"<Error>AccessDenied X-Amz-Signature</Error>",
            1024,
            false,
        ));
        errors.push(storage_case(200, &[1], 1024, true));

        // The site answers the access request without a redirect.
        let server = MockServer::start();
        detail_mock(&server, None);
        server.mock(|when, then| {
            when.method(GET).path("/access-file.aspx");
            then.status(200).body("not a redirect");
        });
        errors.push(
            local_client(&server)
                .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
                .unwrap_err(),
        );

        assert_eq!(errors.len(), 5);
        for e in &errors {
            assert!(matches!(e, ClientError::Attachment(_)), "{e:?}");
            assert_no_secrets(e);
        }
        assert!(violation_files(tmp.path()).is_empty());
    }

    fn fallback_case(alt: Option<&str>, cd: Option<&str>, ct: &str) -> String {
        let server = MockServer::start();
        let storage = MockServer::start();
        detail_mock(&server, alt);
        access_mock(&server, &signed_url(&storage));
        storage.mock(|when, then| {
            when.method(GET).path(SIGNED_PATH);
            let then = then.status(200).header("content-type", ct).body([1u8]);
            if let Some(cd) = cd {
                then.header("content-disposition", cd);
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let saved = local_client(&server)
            .download_violation_attachment(4242, 1234567, tmp.path(), 1024)
            .unwrap();
        saved
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn filename_falls_back_to_content_disposition_then_content_type() {
        assert_eq!(
            fallback_case(
                None,
                Some("attachment; filename=\"Notice 1.pdf\""),
                "application/pdf"
            ),
            "violation-4242-1234567-Notice 1.pdf"
        );
        assert_eq!(
            fallback_case(None, None, "image/png; charset=binary"),
            "violation-4242-1234567-attachment.png"
        );
    }

    #[test]
    fn attachment_name_wins_and_is_sanitised() {
        assert_eq!(
            fallback_case(
                Some("../Front deck.jpg"),
                Some("attachment; filename=\"other.pdf\""),
                "image/jpeg"
            ),
            "violation-4242-1234567-Front deck.jpg"
        );
    }

    #[test]
    fn content_disposition_extended_filename_is_decoded() {
        assert_eq!(
            fallback_case(
                None,
                Some("attachment; filename=\"fallback.pdf\"; filename*=UTF-8''Avis%20%C3%A9t%C3%A9.pdf"),
                "application/pdf"
            ),
            "violation-4242-1234567-Avis été.pdf"
        );
    }

    #[test]
    fn name_without_extension_gets_one_from_content_type() {
        for (name, ct, want) in [
            (
                "hpma kayak agreement",
                "application/pdf",
                "hpma kayak agreement.pdf",
            ),
            ("img 8526", "image/jpeg", "img 8526.jpg"),
            ("baby #2", "audio/mpeg", "baby #2.mp3"),
            ("7", "video/quicktime", "7.mov"),
            ("image (3)", "image/png", "image (3).png"),
            (
                "kayak permit l 6 (yellow easy rider)",
                "image/heic",
                "kayak permit l 6 (yellow easy rider).heic",
            ),
            ("clip", "video/mp4", "clip.mp4"),
            ("log", "text/plain", "log.txt"),
            ("bundle", "application/zip", "bundle.zip"),
            ("letter", "application/msword", "letter.doc"),
            (
                "letter",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "letter.docx",
            ),
            (
                "fines",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "fines.xlsx",
            ),
            ("pic", "image/gif", "pic.gif"),
            ("pic", "image/webp", "pic.webp"),
            // A numeric "extension" that contradicts the type is not one.
            ("img.8526", "image/jpeg", "img.8526.jpg"),
        ] {
            assert_eq!(
                with_type_extension(name.into(), Some(ct)),
                want,
                "{name} {ct}"
            );
        }
        // End to end: the site's extension-less label gets the PDF extension.
        assert_eq!(
            fallback_case(Some("hpma kayak agreement"), None, "application/pdf"),
            "violation-4242-1234567-hpma kayak agreement.pdf"
        );
    }

    #[test]
    fn existing_extension_is_kept() {
        for (name, ct) in [
            ("notice.pdf", "application/pdf"),
            ("Notice.PDF", "application/pdf"),
            ("photo.jpeg", "image/jpeg"),
            ("photo.jpg", "image/jpeg"),
            ("clip.mov", "video/quicktime"),
            ("song.mp3", "audio/mpeg"),
        ] {
            assert_eq!(
                with_type_extension(name.into(), Some(ct)),
                name,
                "{name} {ct}"
            );
        }
        // An extension is kept when the type says nothing to contradict it.
        assert_eq!(with_type_extension("a.pdf".into(), None), "a.pdf");
        assert_eq!(
            with_type_extension("a.pdf".into(), Some("application/octet-stream")),
            "a.pdf"
        );
    }

    #[test]
    fn dot_inside_a_label_is_not_an_extension() {
        assert_eq!(
            with_type_extension("Mr. Smith photo".into(), Some("image/jpeg")),
            "Mr. Smith photo.jpg"
        );
        assert_eq!(
            fallback_case(Some("Mr. Smith photo"), None, "image/jpeg"),
            "violation-4242-1234567-Mr. Smith photo.jpg"
        );
    }

    #[test]
    fn unknown_content_type_adds_no_extension() {
        for ct in [
            None,
            Some("application/octet-stream"),
            Some(""),
            Some("x/\u{0}"),
        ] {
            assert_eq!(with_type_extension("image (3)".into(), ct), "image (3)");
        }
        assert_eq!(
            fallback_case(Some("image (3)"), None, "application/x-unknown"),
            "violation-4242-1234567-image (3)"
        );
    }
}
