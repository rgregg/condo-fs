pub mod calendar;
pub mod library;

use crate::credentials::Credentials;
use crate::error::ClientError;
use crate::model::{Entry, Event, FileMeta};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::io::Write;

pub trait CondoClient {
    fn login(&self) -> Result<(), ClientError>;
    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError>;
    fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError>;
    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError>;
    /// List amenity bookings between two instants, given as Unix seconds.
    fn list_events(&self, start: i64, end: i64) -> Result<Vec<Event>, ClientError>;
}

pub struct HttpCondoClient {
    pub(crate) http: Client,
    pub(crate) base_url: String,
    pub(crate) creds: Credentials,
}

/// A verbatim capture of one GET response, for endpoint discovery.
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub final_url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpCondoClient {
    pub fn new(
        base_url: impl Into<String>,
        creds: Credentials,
    ) -> Result<HttpCondoClient, ClientError> {
        let http = Client::builder()
            .cookie_store(true)
            .redirect(Policy::none()) // we must see 302s to detect auth state
            .user_agent("condo-api/0.1")
            .build()?;
        Ok(HttpCondoClient {
            http,
            base_url: base_url.into(),
            creds,
        })
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Run `op`; if it fails with an expired session, re-authenticate once and retry.
    pub(crate) fn with_reauth<T>(
        &self,
        op: impl Fn() -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        match op() {
            Err(ClientError::Auth) => {
                log::info!("session expired; re-authenticating");
                self.login()?;
                op()
            }
            other => other,
        }
    }

    /// Fetch `path` and capture the response verbatim.
    ///
    /// GET only, by construction — there is no code path here that issues another method.
    /// Redirects are surfaced, not followed, so an expired session is visible in the
    /// capture rather than silently yielding a login page.
    ///
    /// `xhr` adds the `X-Requested-With: XMLHttpRequest` header that Condo Control's JSON
    /// endpoints require.
    pub fn get_raw(&self, path: &str, xhr: bool) -> Result<RawResponse, ClientError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            self.url(path)
        };
        let mut req = self.http.get(&url);
        if xhr {
            req = req.header("X-Requested-With", "XMLHttpRequest");
        }
        let resp = req.send()?;
        let status = resp.status().as_u16();
        let final_url = resp.url().to_string();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or("<non-utf8>").to_string(),
                )
            })
            .collect();
        let body = match resp.bytes() {
            Ok(bytes) => String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| {
                format!(
                    "<non-utf8 body: {} bytes could not be decoded as UTF-8>",
                    e.into_bytes().len()
                )
            }),
            Err(e) => format!("<failed to read body: {e}>"),
        };
        Ok(RawResponse {
            status,
            final_url,
            headers,
            body,
        })
    }
}

impl CondoClient for HttpCondoClient {
    fn login(&self) -> Result<(), ClientError> {
        // 1. GET /login to obtain a session cookie.
        self.http.get(self.url("/login")).send()?;

        // 2. POST credentials as multipart/form-data.
        let form = reqwest::blocking::multipart::Form::new()
            .text("Username", self.creds.username.clone())
            .text("Password", self.creds.password.clone())
            .text("SaveEmail", "false")
            .text("Lang", "en")
            .text("RedirectURL", "");
        let resp = self
            .http
            .post(self.url("/login/login-post"))
            .multipart(form)
            .send()?;

        // Success = 302 to /my/... ; failure = 302 back to /login.
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if location.starts_with("/login") || location.contains("/login?") {
            return Err(ClientError::Auth);
        }
        Ok(())
    }

    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
        self.with_reauth(|| self.list_folder_once(folder_id))
    }

    fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError> {
        self.with_reauth(|| self.file_meta_once(file_id))
    }

    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError> {
        // `out` is `&mut`, which is not `Fn`-friendly, so retry manually.
        match self.download_file_once(file_id, out) {
            Err(ClientError::Auth) => {
                log::info!("session expired; re-authenticating");
                self.login()?;
                self.download_file_once(file_id, out)
            }
            other => other,
        }
    }

    fn list_events(&self, start: i64, end: i64) -> Result<Vec<Event>, ClientError> {
        self.with_reauth(|| self.list_events_once(start, end))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use httpmock::prelude::*;

    pub(crate) fn creds() -> Credentials {
        Credentials {
            username: "u@e.com".into(),
            password: "p@ss^#1".into(),
        }
    }

    #[test]
    fn login_success_sets_cookie_and_returns_ok() {
        let server = MockServer::start();
        let get_login = server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200)
                .header("set-cookie", "ASP.NET_SessionId=abc; path=/")
                .body("<form/>");
        });
        let post_login = server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302)
                .header("location", "/my/my-home")
                .header("set-cookie", "CCCookie=xyz; path=/");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        client.login().unwrap();
        get_login.assert();
        post_login.assert();
    }

    #[test]
    fn login_failure_returns_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/login"); // bounce back = failure
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.login(), Err(ClientError::Auth)));
    }

    #[test]
    fn get_raw_captures_status_headers_and_body() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/announcements/get-announcement-list")
                .header("x-requested-with", "XMLHttpRequest");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"ID":1}]"#);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let raw = client
            .get_raw("/announcements/get-announcement-list", true)
            .unwrap();
        m.assert();
        assert_eq!(raw.status, 200);
        assert_eq!(raw.body, r#"[{"ID":1}]"#);
        assert!(raw
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v.contains("application/json")));
    }

    #[test]
    fn get_raw_reports_redirect_without_following_it() {
        // A 302 to /login means "not authenticated" — the probe must SEE it, not follow it,
        // so an unauthenticated probe run is obvious in the dumped output.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/calendar/get-event-list");
            then.status(302).header("location", "/login?NextPage=x");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let raw = client.get_raw("/calendar/get-event-list", false).unwrap();
        assert_eq!(raw.status, 302);
        assert!(raw
            .headers
            .iter()
            .any(|(k, v)| k == "location" && v.starts_with("/login")));
    }

    #[test]
    fn get_raw_omits_xhr_header_when_not_requested() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/announcements").matches(|req| {
                !req.headers
                    .as_ref()
                    .map(|h| {
                        h.iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("x-requested-with"))
                    })
                    .unwrap_or(false)
            });
            then.status(200).body("<html></html>");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let raw = client.get_raw("/announcements", false).unwrap();
        m.assert();
        assert_eq!(raw.body, "<html></html>");
    }

    #[test]
    fn get_raw_marks_undecodable_body_distinctly_from_empty() {
        // A body that fails to decode as UTF-8 must not read as a genuine empty response —
        // that would be indistinguishable from an endpoint that legitimately returned nothing.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/library/get-file");
            then.status(200)
                .header("content-type", "application/octet-stream")
                .body([0xff, 0xfe, 0xfd]);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let raw = client.get_raw("/library/get-file", false).unwrap();
        assert_eq!(raw.status, 200);
        assert_ne!(raw.body, "");
        assert!(raw.body.contains("non-utf8"));
    }
}
