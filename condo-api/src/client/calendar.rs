use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_event_list, Event};

impl HttpCondoClient {
    /// One attempt at the amenity calendar feed. `start`/`end` are Unix seconds — the
    /// endpoint 302s to an error page for ISO dates or for a missing range.
    ///
    /// There is a timezone asymmetry here that this layer does *not* compensate for:
    /// `start`/`end` are plain Unix seconds with no timezone attached, but the endpoint
    /// evidently interprets them (and reports `Event::start`/`end`) in the association's
    /// own local time, not UTC. A caller that converts calendar dates to Unix seconds at
    /// UTC midnight and uses them as an exact boundary will silently miss or include
    /// bookings near either edge, depending on the association's real UTC offset. The
    /// only current caller, `condo_mcp`'s `list_events` tool
    /// (`condo-mcp/src/tools/calendar.rs`), compensates by widening the requested window
    /// and filtering the parsed results by naive date/range instead of trusting this
    /// call's `start`/`end` as exact cutoffs. Any new caller needs the same treatment;
    /// this method does not do it itself because it has no timezone data to do it with.
    pub(crate) fn list_events_once(&self, start: i64, end: i64) -> Result<Vec<Event>, ClientError> {
        let resp = self
            .http
            .get(self.url("/amenity/json-source/"))
            .header("X-Requested-With", "XMLHttpRequest")
            .query(&[
                ("filter", String::new()),
                ("start", start.to_string()),
                ("end", end.to_string()),
            ])
            .send()?;
        if resp.status().is_redirection() {
            return Err(ClientError::Auth);
        }
        let text = resp.error_for_status()?.text()?;
        Ok(parse_event_list(&text)?)
    }
}

#[cfg(test)]
mod tests {
    use crate::client::tests::creds;
    use crate::{ClientError, CondoClient, HttpCondoClient};
    use httpmock::prelude::*;

    #[test]
    fn list_events_sends_unix_seconds_and_parses_the_feed() {
        let server = MockServer::start();
        let body = std::fs::read_to_string("tests/fixtures/amenity_events.json").unwrap();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/amenity/json-source/")
                .query_param("filter", "")
                .query_param("start", "1754006400")
                .query_param("end", "1756684800")
                .header("x-requested-with", "XMLHttpRequest");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        let events = client.list_events(1754006400, 1756684800).unwrap();
        m.assert();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].id, 7570058);
    }

    #[test]
    fn list_events_302_to_login_is_an_auth_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/amenity/json-source/");
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
        assert!(matches!(client.list_events(0, 1), Err(ClientError::Auth)));
    }

    #[test]
    fn list_events_reauths_once_then_gives_up() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/login");
            then.status(200);
        });
        server.mock(|when, then| {
            when.method(GET).path("/amenity/json-source/");
            then.status(302).header("location", "/login");
        });
        let login_hits = server.mock(|when, then| {
            when.method(POST).path("/login/login-post");
            then.status(302).header("location", "/my/my-home");
        });
        let client = HttpCondoClient::new(server.base_url(), creds()).unwrap();
        assert!(matches!(client.list_events(0, 1), Err(ClientError::Auth)));
        assert_eq!(
            login_hits.hits(),
            1,
            "should re-auth exactly once then give up"
        );
    }
}
