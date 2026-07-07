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

    fn list_folder(&self, _folder_id: u64) -> Result<Vec<Entry>, ClientError> {
        unimplemented!("Task 6")
    }
    fn file_meta(&self, _file_id: u64) -> Result<FileMeta, ClientError> {
        unimplemented!("Task 7")
    }
    fn download_file(&self, _file_id: u64, _out: &mut dyn Write) -> Result<u64, ClientError> {
        unimplemented!("Task 7")
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
}
