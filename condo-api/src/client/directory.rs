use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_directory_page, DirectoryPage, DirectoryQuery};
use reqwest::StatusCode;

impl HttpCondoClient {
    /// One attempt at a page of the owner directory.
    ///
    /// The SPA's phone-book page calls `/web/PhoneBook/Directory`; `/web` is its core API
    /// base. A redirect or 401/403 means the session is gone.
    pub(crate) fn list_directory_once(
        &self,
        q: &DirectoryQuery,
    ) -> Result<DirectoryPage, ClientError> {
        let mut params = vec![
            ("page", q.page.to_string()),
            ("pageSize", q.page_size.to_string()),
        ];
        if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
            params.push(("searchTerm", s.to_string()));
        }
        let resp = self
            .http
            .get(self.url("/web/PhoneBook/Directory"))
            .header("Accept", "application/json")
            .query(&params)
            .send()?;
        if resp.status().is_redirection()
            || matches!(
                resp.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            )
        {
            return Err(ClientError::Auth);
        }
        let body = resp.error_for_status()?.text()?;
        Ok(parse_directory_page(&body)?)
    }
}

#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{ClientError, DirectoryApi, DirectoryQuery, HttpCondoClient};
    use httpmock::prelude::*;

    fn query(search: Option<&str>) -> DirectoryQuery {
        DirectoryQuery {
            search: search.map(str::to_string),
            page: 2,
            page_size: 100,
        }
    }

    #[test]
    fn sends_paging_and_search_term_and_parses() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/directory.json").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/web/PhoneBook/Directory")
                .query_param("page", "2")
                .query_param("pageSize", "100")
                .query_param("searchTerm", "Example")
                .header("accept", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let page = client.list_directory(&query(Some("Example"))).unwrap();
        m.assert();
        assert_eq!(page.entries.len(), 4);
    }

    #[test]
    fn omits_search_term_when_empty() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/directory.json").unwrap();
        let with_term = server.mock(|when, then| {
            when.method(GET)
                .path("/web/PhoneBook/Directory")
                .query_param_exists("searchTerm");
            then.status(500);
        });
        let ok = server.mock(|when, then| {
            when.method(GET).path("/web/PhoneBook/Directory");
            then.status(200).body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        client.list_directory(&query(Some(""))).unwrap();
        ok.assert();
        assert_eq!(with_term.hits(), 0);
    }

    #[test]
    fn redirect_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/web/PhoneBook/Directory");
            then.status(302).header("location", "/login?NextPage=x");
        });
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/login");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.list_directory(&query(None)),
            Err(ClientError::Auth)
        ));
    }

    #[test]
    fn a_non_json_body_is_a_parse_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/web/PhoneBook/Directory");
            then.status(200).body("<html>Invalid address</html>");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.list_directory(&query(None)),
            Err(ClientError::Parse(_))
        ));
    }
}
