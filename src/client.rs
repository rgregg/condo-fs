use crate::credentials::Credentials;
#[allow(unused_imports)]
use crate::model::{parse_file_list, Entry, FileMeta};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use std::io::Write;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("authentication failed")]
    Auth,
    #[error("parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found")]
    NotFound,
}

pub trait CondoClient {
    fn login(&self) -> Result<(), ClientError>;
    fn list_folder(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError>;
    fn file_meta(&self, file_id: u64) -> Result<FileMeta, ClientError>;
    fn download_file(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError>;
}

pub struct HttpCondoClient {
    http: Client,
    base_url: String,
    creds: Credentials,
}

impl HttpCondoClient {
    pub fn new(
        base_url: impl Into<String>,
        creds: Credentials,
    ) -> Result<HttpCondoClient, ClientError> {
        let http = Client::builder()
            .cookie_store(true)
            .redirect(Policy::none()) // we must see 302s to detect auth state
            .user_agent("condo-fuse/0.1")
            .build()?;
        Ok(HttpCondoClient {
            http,
            base_url: base_url.into(),
            creds,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Run `op`; if it fails with an expired session, re-authenticate once and retry.
    fn with_reauth<T>(
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

    fn list_folder_once(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
        let resp = self
            .http
            .get(self.url("/library/get-file-list"))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(&[
                ("mode", "0".to_string()),
                ("folderID", folder_id.to_string()),
                ("searchString", String::new()),
                ("fileTypeSelectID", "0".to_string()),
                ("startDate", String::new()),
                ("endDate", String::new()),
                ("newSearch", "False".to_string()),
            ])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let text = resp.error_for_status()?.text()?;
        Ok(parse_file_list(&text)?)
    }

    fn file_meta_once(&self, file_id: u64) -> Result<FileMeta, ClientError> {
        // GET but read only the headers; drop the response without consuming the body.
        let resp = self
            .http
            .get(self.url("/library/download-file"))
            .query(&[("fileRecordID", file_id.to_string())])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let resp = resp.error_for_status()?;
        let size = resp.content_length().unwrap_or(0);
        let filename = resp
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_disposition_filename);
        // resp dropped here without reading the body.
        Ok(FileMeta { size, filename })
    }

    fn download_file_once(&self, file_id: u64, out: &mut dyn Write) -> Result<u64, ClientError> {
        let resp = self
            .http
            .get(self.url("/library/download-file"))
            .query(&[("fileRecordID", file_id.to_string())])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let mut resp = resp.error_for_status()?;
        let n = resp.copy_to(out)?;
        Ok(n)
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
}

fn parse_content_disposition_filename(header: &str) -> Option<String> {
    // e.g. attachment; filename="01/09/25 Board Minutes.pdf"
    let idx = header.to_ascii_lowercase().find("filename=")?;
    let rest = &header[idx + "filename=".len()..];
    let rest = rest.trim();
    let name = rest
        .strip_prefix('"')
        .and_then(|s| s.split('"').next())
        .unwrap_or(rest);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn creds() -> Credentials {
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
    fn list_folder_parses_entries() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/files.json").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/library/get-file-list")
                .query_param("folderID", "262667")
                .query_param("mode", "0")
                .query_param("newSearch", "False")
                .header("x-requested-with", "XMLHttpRequest");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let entries = client.list_folder(262667).unwrap();
        m.assert();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], Entry::File { id: 5369528, .. }));
    }

    #[test]
    fn list_folder_302_to_login_is_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/library/get-file-list");
            then.status(302).header("location", "/login?NextPage=x");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.list_folder(1), Err(ClientError::Auth)));
    }

    #[test]
    fn file_meta_reads_length_and_filename() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/library/download-file")
                .query_param("fileRecordID", "5369528");
            then.status(200)
                .header("content-length", "279033")
                .header(
                    "content-disposition",
                    "attachment; filename=\"01/09/25 Board Minutes.pdf\"",
                )
                .header("content-type", "application/pdf")
                .body(vec![0u8; 279033]);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let meta = client.file_meta(5369528).unwrap();
        m.assert();
        assert_eq!(meta.size, 279033);
        assert_eq!(meta.filename.as_deref(), Some("01/09/25 Board Minutes.pdf"));
    }

    #[test]
    fn download_file_writes_all_bytes() {
        let server = MockServer::start();
        let payload = b"%PDF-1.7 hello".to_vec();
        server.mock(|when, then| {
            when.method(GET)
                .path("/library/download-file")
                .query_param("fileRecordID", "42");
            then.status(200).body(payload.clone());
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let n = client.download_file(42, &mut buf).unwrap();
        assert_eq!(n as usize, payload.len());
        assert_eq!(buf, payload);
    }

    #[test]
    fn list_folder_reauths_once_then_gives_up() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        // get-file-list always reports an expired session.
        server.mock(|when, then| {
            when.method(GET)
                .path("/library/get-file-list")
                .query_param("folderID", "9");
            then.status(302).header("location", "/login");
        });
        // Count login attempts: re-auth must happen exactly once, not loop forever.
        let login_hits = server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let res = client.list_folder(9);
        assert!(matches!(res, Err(ClientError::Auth)));
        assert_eq!(login_hits.hits(), 1, "should re-auth exactly once then give up");
    }
}
