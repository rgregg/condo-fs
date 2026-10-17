use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{
    parse_violation_detail, parse_violation_filters, parse_violation_page, ViolationDetail,
    ViolationFilterOptions, ViolationPage, ViolationQuery,
};

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
}

#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{ClientError, HttpCondoClient, ViolationQuery, ViolationStatus, ViolationsApi};
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
                .path("/violationTracking/view-violation-details/885")
                .query_param("status", "-1");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let d = client.get_violation(885).unwrap();
        m.assert();
        assert_eq!(d.subject, "RV Storage No Current Sticker");
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
                .path("/violationTracking/view-violation-details/885");
            then.status(302).header("location", "/login?NextPage=x");
        });
        login_mocks(&server);
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.get_violation(885), Err(ClientError::Auth)));
    }

    #[test]
    fn get_violation_200_shell_is_a_structure_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/violationTracking/view-violation-details/885");
            then.status(200)
                .body("<html><body>Invalid address</body></html>");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(
            client.get_violation(885),
            Err(ClientError::PageStructure(_))
        ));
    }
}
