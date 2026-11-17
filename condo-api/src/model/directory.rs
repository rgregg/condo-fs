use serde::Deserialize;

/// A phone number as the directory lists it. `kind` is the site's label (Cell, Home, Other).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPhone {
    pub number: String,
    pub kind: String,
}

/// One row of the owner directory. One owner can appear on several rows (one per unit or
/// resident record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub unit: String,
    pub first_name: String,
    pub last_name: String,
    pub phones: Vec<DirectoryPhone>,
    pub email: Option<String>,
    /// The on-site mailing address, one string per line. Empty when the site has none.
    pub on_site_address: Vec<String>,
}

/// One page of the directory plus the site's paging totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPage {
    pub entries: Vec<DirectoryEntry>,
    pub page: u32,
    pub page_size: u32,
    pub total_items: u32,
    pub total_pages: u32,
    pub has_next_page: bool,
}

/// What to ask the directory for. `page` is 1-based; the site rejects a `page_size` far
/// above 100 with HTTP 400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryQuery {
    /// Matches unit and name on the site; email, phone and street are not searched.
    pub search: Option<String>,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Deserialize)]
struct RawPhone {
    number: String,
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Deserialize)]
struct RawAddress {
    #[serde(default)]
    lines: Vec<String>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "unitName")]
    unit_name: String,
    #[serde(rename = "firstName", default)]
    first_name: String,
    #[serde(rename = "lastName", default)]
    last_name: String,
    #[serde(rename = "phoneNumbers", default)]
    phone_numbers: Option<Vec<RawPhone>>,
    #[serde(rename = "emailAddress", default)]
    email_address: Option<String>,
    #[serde(rename = "onSiteAddress", default)]
    on_site_address: Option<RawAddress>,
}

#[derive(Deserialize)]
struct RawPagination {
    page: u32,
    #[serde(rename = "pageSize")]
    page_size: u32,
    #[serde(rename = "totalItems")]
    total_items: u32,
    #[serde(rename = "totalPages")]
    total_pages: u32,
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
struct RawPage {
    data: Vec<RawEntry>,
    pagination: RawPagination,
}

fn non_blank(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Parse one page of `GET /web/PhoneBook/Directory`.
///
/// Missing or null phone list, email and address are normal and become empty values.
/// `offSiteAddress` was null in every probed row and its shape is unknown, so it is ignored
/// rather than guessed at.
pub fn parse_directory_page(json: &str) -> Result<DirectoryPage, serde_json::Error> {
    let raw: RawPage = serde_json::from_str(json)?;
    let entries = raw
        .data
        .into_iter()
        .map(|r| DirectoryEntry {
            unit: r.unit_name.trim().to_string(),
            first_name: r.first_name.trim().to_string(),
            last_name: r.last_name.trim().to_string(),
            phones: r
                .phone_numbers
                .unwrap_or_default()
                .into_iter()
                .map(|p| DirectoryPhone {
                    number: p.number.trim().to_string(),
                    kind: p.kind,
                })
                .filter(|p| !p.number.is_empty())
                .collect(),
            email: non_blank(r.email_address),
            on_site_address: r
                .on_site_address
                .map(|a| a.lines)
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        })
        .collect();
    Ok(DirectoryPage {
        entries,
        page: raw.pagination.page,
        page_size: raw.pagination.page_size,
        total_items: raw.pagination.total_items,
        total_pages: raw.pagination.total_pages,
        has_next_page: raw.pagination.has_next_page,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string("tests/fixtures/directory.json").unwrap()
    }

    #[test]
    fn parses_entries_and_paging() {
        let page = parse_directory_page(&fixture()).unwrap();
        assert_eq!(page.entries.len(), 4);
        assert_eq!(page.total_items, 4);
        assert_eq!(page.page_size, 25);
        assert!(!page.has_next_page);
        let a = &page.entries[0];
        assert_eq!(a.unit, "101");
        assert_eq!(a.first_name, "Alex");
        assert_eq!(a.phones.len(), 2);
        assert_eq!(a.phones[1].kind, "Home");
        assert_eq!(a.email.as_deref(), Some("alex.example@example.com"));
        assert_eq!(
            a.on_site_address,
            vec!["101 Sample Lane", "Testville, ZZ 00000"]
        );
    }

    #[test]
    fn missing_phone_email_and_address_are_empty_not_errors() {
        let page = parse_directory_page(&fixture()).unwrap();
        let b = &page.entries[1];
        assert!(b.phones.is_empty());
        assert!(b.on_site_address.is_empty());
        let c = &page.entries[2];
        assert_eq!(c.email, None);
    }

    #[test]
    fn null_phone_list_and_blank_email_are_tolerated() {
        let json = r#"{"data":[{"unitName":"9","firstName":"A","lastName":"B",
            "phoneNumbers":null,"emailAddress":"  ","onSiteAddress":null}],
            "pagination":{"page":1,"pageSize":25,"totalItems":1,"totalPages":1,
            "hasNextPage":false,"hasPreviousPage":false}}"#;
        let e = &parse_directory_page(json).unwrap().entries[0];
        assert!(e.phones.is_empty());
        assert_eq!(e.email, None);
    }

    #[test]
    fn a_page_without_data_is_a_parse_error() {
        assert!(parse_directory_page(r#"{"rows":[]}"#).is_err());
        assert!(parse_directory_page("<html>login</html>").is_err());
    }
}
