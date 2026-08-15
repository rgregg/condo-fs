use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_announcement_list, AnnouncementSummary};

impl HttpCondoClient {
    /// One attempt at the announcements list page.
    ///
    /// Returns the default view only: currently-posted, non-expired announcements. The
    /// archive is reachable solely through a POST filter form, which this read-only client
    /// will not submit.
    pub(crate) fn list_announcements_once(&self) -> Result<Vec<AnnouncementSummary>, ClientError> {
        let resp = self.http.get(self.url("/announcement/show-list")).send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let html = resp.error_for_status()?.text()?;
        Ok(parse_announcement_list(&html)?)
    }
}

#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{ClientError, CondoClient, HttpCondoClient};
    use httpmock::prelude::*;

    #[test]
    fn list_announcements_fetches_and_parses_the_page() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/announcements_list.html").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET).path("/announcement/show-list");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let items = client.list_announcements().unwrap();
        m.assert();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, 1541631);
    }

    #[test]
    fn list_announcements_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/announcement/show-list");
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
            client.list_announcements(),
            Err(ClientError::Auth)
        ));
    }

    #[test]
    fn a_page_without_the_table_is_a_structure_error() {
        // The "Invalid address" shell returns HTTP 200, so status alone cannot detect it.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/announcement/show-list");
            then.status(200)
                .body("<html><body>Invalid address</body></html>");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.list_announcements(),
            Err(ClientError::PageStructure(_))
        ));
    }
}
