use crate::client::HttpCondoClient;
use crate::error::ClientError;
use crate::model::{parse_file_list, Entry, FileMeta};
use std::io::Write;

impl HttpCondoClient {
    pub(crate) fn list_folder_once(&self, folder_id: u64) -> Result<Vec<Entry>, ClientError> {
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

    pub(crate) fn file_meta_once(&self, file_id: u64) -> Result<FileMeta, ClientError> {
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

    pub(crate) fn download_file_once(
        &self,
        file_id: u64,
        out: &mut dyn Write,
    ) -> Result<u64, ClientError> {
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
    use crate::client::tests::creds;
    use crate::client::LibraryApi;
    use httpmock::prelude::*;

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
        assert_eq!(
            login_hits.hits(),
            1,
            "should re-auth exactly once then give up"
        );
    }
}
