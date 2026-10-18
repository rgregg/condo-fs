use crate::error::{ClientError, ParseError};
use crate::model::{announcements::parse_us_datetime, html};
use chrono::{NaiveDate, NaiveDateTime};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use std::path::PathBuf;

/// The status filter of the violations list, with the ids the site's own dropdown uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationStatus {
    Open,
    Closed,
    Draft,
    PastDue,
    Void,
    ResidentUpdated,
    All,
}

impl ViolationStatus {
    /// The `SelectedViolationStatusID` value the site expects.
    pub fn site_id(self) -> i32 {
        match self {
            ViolationStatus::Open => 1,
            ViolationStatus::Closed => 3,
            ViolationStatus::Draft => 4,
            ViolationStatus::PastDue => -10,
            ViolationStatus::Void => 5,
            ViolationStatus::ResidentUpdated => -20,
            ViolationStatus::All => -1,
        }
    }
}

/// A page request against the violations list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationQuery {
    pub status: ViolationStatus,
    pub search: Option<String>,
    pub type_ids: Vec<u64>,
    pub stage_ids: Vec<u64>,
    pub offset: u32,
    pub limit: u32,
}

/// One row of the violations list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationSummary {
    pub id: u64,
    pub name: String,
    pub violation_type: String,
    pub unit: String,
    pub stage: String,
    pub due_date: Option<NaiveDate>,
    pub last_updated: Option<NaiveDate>,
    /// The user name the site appends after the `LastUpdated` date, when present.
    pub last_updated_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationPage {
    /// Total matching rows across all pages.
    pub total: u64,
    pub rows: Vec<ViolationSummary>,
}

/// Ids and labels the list page offers in its filter dropdowns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationFilterOptions {
    pub workspace_id: u64,
    pub types: Vec<(u64, String)>,
    /// Stage names are not unique: the site lists the same label under several ids.
    pub stages: Vec<(u64, String)>,
}

#[derive(Deserialize)]
struct RawPage {
    total: u64,
    rows: Vec<RawRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawRow {
    violation_id: u64,
    violation_name: String,
    violation_type: String,
    unit: String,
    stage: String,
    due_date: Option<String>,
    last_updated: Option<String>,
}

fn parse_us_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%m/%d/%Y").ok()
}

/// Split `10/17/2026 admin` into the date and the trailing user name; a bare date is fine.
fn split_last_updated(s: &str) -> (Option<NaiveDate>, Option<String>) {
    let s = s.trim();
    let (date, by) = match s.split_once(char::is_whitespace) {
        Some((d, rest)) => (d, rest.trim()),
        None => (s, ""),
    };
    let by = (!by.is_empty()).then(|| by.to_string());
    (parse_us_date(date), by)
}

/// Parse the `search-violation-new` JSON. Malformed or reshaped JSON is an error; an empty
/// `rows` array is a genuinely empty page.
pub fn parse_violation_page(json: &str) -> Result<ViolationPage, ClientError> {
    let raw: RawPage = serde_json::from_str(json)?;
    let rows = raw
        .rows
        .into_iter()
        .map(|r| {
            let (last_updated, last_updated_by) = r
                .last_updated
                .as_deref()
                .map(split_last_updated)
                .unwrap_or((None, None));
            ViolationSummary {
                id: r.violation_id,
                name: r.violation_name.trim().to_string(),
                violation_type: r.violation_type.trim().to_string(),
                unit: r.unit.trim().to_string(),
                stage: r.stage.trim().to_string(),
                due_date: r.due_date.as_deref().and_then(parse_us_date),
                last_updated,
                last_updated_by,
            }
        })
        .collect();
    Ok(ViolationPage {
        total: raw.total,
        rows,
    })
}

fn select_options(doc: &Html, select_css: &'static str) -> Result<Vec<(u64, String)>, ParseError> {
    let sel = Selector::parse(select_css).unwrap();
    let opt = Selector::parse("option").unwrap();
    let select = doc
        .select(&sel)
        .next()
        .ok_or(ParseError::MissingElement(select_css))?;
    Ok(select
        .select(&opt)
        .filter_map(|o| {
            let id = o.value().attr("value")?.trim().parse::<u64>().ok()?;
            Some((id, o.text().collect::<String>().trim().to_string()))
        })
        .collect())
}

/// Read the workspace id and the type/stage dropdown options from the list page.
///
/// Anchored on `#WorkspaceID`, `#DDLViolationType` and `#DDLViolationAction`. A missing
/// element is an error; an empty dropdown is an empty list.
pub fn parse_violation_filters(html: &str) -> Result<ViolationFilterOptions, ParseError> {
    let doc = Html::parse_document(html);
    let ws_sel = Selector::parse("input#WorkspaceID").unwrap();
    let workspace_id = doc
        .select(&ws_sel)
        .next()
        .and_then(|e| e.value().attr("value"))
        .ok_or(ParseError::MissingElement("input#WorkspaceID"))?
        .trim()
        .parse::<u64>()
        .map_err(|_| ParseError::MissingElement("input#WorkspaceID numeric value"))?;
    Ok(ViolationFilterOptions {
        workspace_id,
        types: select_options(&doc, "select#DDLViolationType")?,
        stages: select_options(&doc, "select#DDLViolationAction")?,
    })
}

/// A file referenced from an update body (`access-file.aspx?FileRecordID=…&Key=…`).
///
/// The `Key` is a bearer-style secret in the URL: it is kept crate-private, redacted from
/// `Debug`, and stripped from `body_markdown`, so it cannot leak through logs or tool output.
#[derive(Clone, PartialEq, Eq)]
pub struct ViolationAttachment {
    pub id: u64,
    pub name: Option<String>,
    pub(crate) key: String,
}

impl std::fmt::Debug for ViolationAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViolationAttachment")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("key", &"<redacted>")
            .finish()
    }
}

/// Where a downloaded attachment was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAttachment {
    /// Canonical path of the saved file, always inside the requested folder.
    pub path: PathBuf,
    pub bytes: u64,
    /// The storage response's media type (parameters dropped), if it sent one.
    pub content_type: Option<String>,
}

/// One entry of a violation's update timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationUpdate {
    /// 1 = oldest.
    pub ordinal: usize,
    /// A stable site identifier for the update. The page exposes none today, so `None`.
    pub site_id: Option<String>,
    pub author: String,
    pub posted_at: Option<NaiveDateTime>,
    pub body_markdown: String,
    pub attachments: Vec<ViolationAttachment>,
}

/// A violation's metadata and its updates, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationDetail {
    pub id: u64,
    pub subject: String,
    pub unit: Option<String>,
    pub recipients: Option<String>,
    pub violation_type: Option<String>,
    pub created_by: Option<String>,
    pub stage: Option<String>,
    pub violation_date: Option<NaiveDateTime>,
    pub due_date: Option<NaiveDateTime>,
    pub updates: Vec<ViolationUpdate>,
}

/// Update timestamps carry no seconds, e.g. `08/18/2026 10:52 AM`.
fn parse_update_datetime(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%Y %l:%M %p").ok()
}

/// Pull `(FileRecordID, Key)` out of an `access-file.aspx` URL.
fn parse_access_file_url(url: &str) -> Option<(u64, String)> {
    let query = url.split_once("access-file.aspx?")?.1;
    let (mut id, mut key) = (None, None);
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some((k, v)) if k.eq_ignore_ascii_case("FileRecordID") => {
                id = v.trim().parse::<u64>().ok()
            }
            Some((k, v)) if k.eq_ignore_ascii_case("Key") => key = Some(v.trim().to_string()),
            _ => {}
        }
    }
    Some((id?, key.filter(|k| !k.is_empty())?))
}

fn parse_violation_update(
    body: ElementRef<'_>,
) -> Result<(String, Vec<ViolationAttachment>), ParseError> {
    let sel = Selector::parse("img, a").unwrap();
    let mut attachments: Vec<ViolationAttachment> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for el in body.select(&sel) {
        let v = el.value();
        let url = if v.name() == "img" {
            v.attr("src")
        } else {
            v.attr("href")
        };
        let Some(url) = url.filter(|u| u.contains("access-file.aspx")) else {
            continue;
        };
        let Some((id, key)) = parse_access_file_url(url) else {
            continue;
        };
        urls.push(url.to_string());
        if attachments.iter().any(|a| a.id == id) {
            continue;
        }
        let name = v
            .attr("alt")
            .or_else(|| v.attr("title"))
            .map(str::to_string)
            .or_else(|| Some(el.text().collect::<String>()))
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            // A label that is the URL itself (an editor autolink) would carry the key.
            .filter(|n| {
                let l = n.to_lowercase();
                !(l.contains("access-file")
                    || l.contains("key=")
                    || l.contains(&key.to_lowercase()))
            });
        attachments.push(ViolationAttachment { id, name, key });
    }
    let mut markdown = html::to_markdown(&body.inner_html(), "td.divUpdateText")?;
    // The URL carries the key; replace it with an opaque reference to the attachment id.
    for url in &urls {
        if let Some((id, _)) = parse_access_file_url(url) {
            markdown = markdown.replace(url.as_str(), &format!("condo-attachment:{id}"));
        }
    }
    for a in &attachments {
        for k in [a.key.clone(), a.key.to_lowercase(), a.key.to_uppercase()] {
            markdown = markdown.replace(&k, "REDACTED");
        }
    }
    Ok((scrub_keys(&markdown).trim().to_string(), attachments))
}

/// Replace `text[start..end]`-style runs, matching `needle` ASCII case-insensitively and
/// ending each run where `stop` first returns true (or at the end of the text).
fn scrub_runs(text: &str, needle: &str, keep: &str, stop: impl Fn(char) -> bool) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while let Some(found) = lower[pos..].find(&needle) {
        let start = pos + found + needle.len();
        out.push_str(&text[pos..start]);
        out.push_str(keep);
        let run_end = text[start..].find(&stop).map_or(text.len(), |i| start + i);
        pos = run_end;
    }
    out.push_str(&text[pos..]);
    out
}

/// Unconditional last line of defence: whatever the markup looked like and whether or not
/// the URL parsed as an attachment, no `access-file.aspx?...` query and no `Key=<token>`
/// may survive in the Markdown.
fn scrub_keys(markdown: &str) -> String {
    let no_urls = scrub_runs(markdown, "access-file.aspx?", "REDACTED", |c| {
        c.is_whitespace() || matches!(c, ')' | '>' | '"' | '\'' | ']' | '<')
    });
    scrub_runs(&no_urls, "key=", "REDACTED", |c| {
        !(c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// Parse a violation detail page.
///
/// Metadata is found by label text (`label.control-label`), never by position. The page lists
/// updates newest first inside `#updateRows`; the result is oldest first with 1-based
/// ordinals. A page with no `Subject:` label or no `#updateRows` container (including the
/// "Invalid address" shell, which arrives as HTTP 200) is a structure error, but a present
/// container with no update tables is a legitimately empty timeline.
pub fn parse_violation_detail(id: u64, page: &str) -> Result<ViolationDetail, ParseError> {
    let doc = Html::parse_fragment(page);
    let value = |label: &str| html::labelled_value(&doc, label, "span.textLeft");
    let date = |label: &str| value(label).and_then(|s| parse_us_datetime(&s));

    let subject =
        value("Subject:").ok_or(ParseError::MissingElement("label.control-label:Subject:"))?;

    let rows_sel = Selector::parse("#updateRows").unwrap();
    let rows = doc
        .select(&rows_sel)
        .next()
        .ok_or(ParseError::MissingElement("#updateRows"))?;
    let table_sel = Selector::parse("table").unwrap();
    let td_sel = Selector::parse("td").unwrap();
    let author_sel = Selector::parse("span").unwrap();
    let mut updates = Vec::new();
    for table in rows.select(&table_sel) {
        // Skip tables nested inside an update body: only top-level update tables count.
        if table
            .ancestors()
            .filter_map(ElementRef::wrap)
            .take_while(|a| a.id() != rows.id())
            .any(|a| a.value().name() == "table")
        {
            continue;
        }
        // A top-level table without a body cell means the markup moved (e.g. the class was
        // renamed). Skipping it would report "no updates", which is a different statement.
        let body = table
            .select(&td_sel)
            .find(|td| td.value().classes().any(|c| c == "divUpdateText"))
            .ok_or(ParseError::MissingElement("td.divUpdateText"))?;
        let meta = table
            .select(&td_sel)
            .find(|td| !td.value().classes().any(|c| c == "divUpdateText"));
        let author = meta
            .and_then(|m| m.select(&author_sel).next())
            .map(|a| a.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let posted_at = meta.and_then(|m| m.text().find_map(parse_update_datetime));
        let (body_markdown, attachments) = parse_violation_update(body)?;
        updates.push(ViolationUpdate {
            ordinal: 0,
            site_id: None,
            author,
            posted_at,
            body_markdown,
            attachments,
        });
    }
    updates.reverse();
    for (i, u) in updates.iter_mut().enumerate() {
        u.ordinal = i + 1;
    }

    Ok(ViolationDetail {
        id,
        subject,
        unit: value("Unit Involved in the Violation:"),
        recipients: value("Recipients:"),
        violation_type: value("Violation Type:"),
        created_by: value("Created By:"),
        stage: value("Violation Stage:"),
        violation_date: date("Violation Date:"),
        due_date: date("Due Date:"),
        updates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/{name}")).unwrap()
    }

    #[test]
    fn parses_rows_and_total() {
        let page = parse_violation_page(&fixture("violations_search.json")).unwrap();
        assert_eq!(page.total, 918);
        assert_eq!(page.rows.len(), 3);
        let first = &page.rows[0];
        assert_eq!(first.id, 107330);
        assert_eq!(first.stage, "30 Day Warning");
        assert_eq!(first.unit, "101");
        assert_eq!(first.due_date, NaiveDate::from_ymd_opt(2026, 10, 18));
        assert_eq!(
            page.rows[1].stage,
            "Proposed Fine/Option for Appeal Hearing"
        );
        assert_eq!(page.rows[1].name, "Vessel Seaworthiness");
    }

    #[test]
    fn last_updated_with_trailing_user_is_split() {
        let row = |lu: &str| {
            format!(
                r#"{{"total":1,"rows":[{{"ViolationName":"n","ViolationType":"t","Unit":"1","Stage":"s","DueDate":"","LastUpdated":"{lu}","ViolationId":5}}]}}"#
            )
        };
        let p = parse_violation_page(&row("10/17/2026 admin")).unwrap();
        assert_eq!(
            p.rows[0].last_updated,
            NaiveDate::from_ymd_opt(2026, 10, 17)
        );
        assert_eq!(p.rows[0].last_updated_by.as_deref(), Some("admin"));
        assert_eq!(p.rows[0].due_date, None);
        let p = parse_violation_page(&row("10/17/2026")).unwrap();
        assert_eq!(
            p.rows[0].last_updated,
            NaiveDate::from_ymd_opt(2026, 10, 17)
        );
        assert_eq!(p.rows[0].last_updated_by, None);
    }

    #[test]
    fn empty_rows_is_an_empty_page_not_an_error() {
        let p = parse_violation_page(r#"{"total":0,"rows":[]}"#).unwrap();
        assert!(p.rows.is_empty());
        assert_eq!(p.total, 0);
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        assert!(matches!(
            parse_violation_page("<html>nope</html>"),
            Err(ClientError::Parse(_))
        ));
        assert!(matches!(
            parse_violation_page(r#"{"items":[]}"#),
            Err(ClientError::Parse(_))
        ));
    }

    #[test]
    fn filters_page_yields_workspace_type_and_stage_ids() {
        let f = parse_violation_filters(&fixture("violations_list_page.html")).unwrap();
        assert_eq!(f.workspace_id, 13582);
        assert!(f
            .types
            .contains(&(11377, "Article IV Land Use Restrictions".to_string())));
        // Trailing space in the site's label is trimmed; entities are decoded.
        assert!(f
            .types
            .contains(&(11381, "Article III Architectural Control".to_string())));
        assert!(f.types.contains(&(
            14875,
            "CC&R's Article VIII Building Restrictions".to_string()
        )));
        assert!(f.stages.contains(&(8418, "Warning".to_string())));
        assert!(f
            .stages
            .contains(&(8420, "Owner Declines Hearing".to_string())));
        assert!(f.stages.contains(&(10233, "30 Day Warning".to_string())));
        // The status dropdown is not mistaken for either list.
        assert!(!f.types.iter().any(|(_, l)| l == "Open"));
        assert!(!f.stages.iter().any(|(_, l)| l == "Open"));

        assert!(matches!(
            parse_violation_filters("<html><body>Invalid address</body></html>"),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn filters_page_missing_a_dropdown_is_a_structure_error() {
        let html = r#"<input id="WorkspaceID" value="7"><select id="DDLViolationType"></select>"#;
        assert!(matches!(
            parse_violation_filters(html),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn status_site_ids_match_the_site() {
        use ViolationStatus::*;
        assert_eq!(Open.site_id(), 1);
        assert_eq!(Closed.site_id(), 3);
        assert_eq!(Draft.site_id(), 4);
        assert_eq!(PastDue.site_id(), -10);
        assert_eq!(Void.site_id(), 5);
        assert_eq!(ResidentUpdated.site_id(), -20);
        assert_eq!(All.site_id(), -1);
    }

    const FAKE_KEY: &str = "00000000-0000-0000-0000-000000000000";

    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    #[test]
    fn metadata_is_read_by_label_text() {
        let d = parse_violation_detail(4242, &fixture("violation_detail.html")).unwrap();
        assert_eq!(d.id, 4242);
        assert_eq!(d.subject, "Unscreened Boat Trailer Example");
        assert_eq!(d.unit.as_deref(), Some("101"));
        assert_eq!(d.recipients.as_deref(), Some("Owner, Fake Mr. (101)"));
        assert_eq!(
            d.violation_type.as_deref(),
            Some("Article IV Land Use Restrictions")
        );
        assert_eq!(d.created_by.as_deref(), Some("HPMA Compliance"));
        assert_eq!(
            d.stage.as_deref(),
            Some("Proposed Fine/Option for Appeal Hearing")
        );
        assert_eq!(d.violation_date, Some(dt(2026, 3, 2, 9, 15, 30)));
        assert_eq!(d.due_date, Some(dt(2026, 4, 3, 10, 0, 0)));
    }

    #[test]
    fn updates_are_oldest_first_with_ordinals() {
        let d = parse_violation_detail(4242, &fixture("violation_detail.html")).unwrap();
        assert_eq!(d.updates.len(), 3);
        assert_eq!(
            d.updates.iter().map(|u| u.ordinal).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(d.updates[0].posted_at, Some(dt(2026, 3, 4, 8, 5, 0)));
        assert_eq!(d.updates[2].posted_at, Some(dt(2026, 3, 20, 11, 20, 0)));
        assert!(d
            .updates
            .windows(2)
            .all(|w| w[0].posted_at <= w[1].posted_at));
        assert_eq!(d.updates[0].author, "Compliance, Fake (202)");
        assert_eq!(d.updates[0].site_id, None);
    }

    #[test]
    fn update_body_is_markdown_and_keeps_inline_image_attachment() {
        let d = parse_violation_detail(4242, &fixture("violation_detail.html")).unwrap();
        let first = &d.updates[0];
        assert_eq!(first.attachments.len(), 1);
        assert_eq!(first.attachments[0].id, 1234567);
        assert_eq!(first.attachments[0].key, FAKE_KEY);
        assert!(!first.body_markdown.contains(FAKE_KEY));
        assert!(!first.body_markdown.contains("Key="));
        assert!(!first.body_markdown.contains("<p>"));
        assert!(first
            .body_markdown
            .contains("**Example Violation Heading:**"));
        assert!(d.updates[1].attachments.is_empty());
    }

    #[test]
    fn zero_updates_is_an_empty_timeline() {
        let d = parse_violation_detail(1, &fixture("violation_detail_empty.html")).unwrap();
        assert!(d.updates.is_empty());
        assert_eq!(d.subject, "Unscreened Boat Trailer Example");
        assert_eq!(d.unit.as_deref(), Some("101"));
    }

    #[test]
    fn the_invalid_address_shell_is_a_structure_error() {
        assert!(matches!(
            parse_violation_detail(1, "<html><body>Invalid address</body></html>"),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn markup_reordered_still_parses() {
        let html = fixture("violation_detail.html");
        let start = html.find("<div id=\"metadata\">").unwrap() + "<div id=\"metadata\">".len();
        let end = html.find("<div id=\"divNextWarning\"").unwrap();
        let block = &html[start..end];
        let block = &block[..block.rfind("</div>").unwrap()];
        let mut groups: Vec<&str> = block
            .split("<div class=\"form-group\">")
            .filter(|g| g.contains("control-label"))
            .collect();
        groups.reverse();
        let reordered: String = groups
            .iter()
            .map(|g| format!("<div class=\"form-group\">{g}"))
            .collect();
        let html = format!(
            "{}{}{}",
            &html[..start],
            reordered,
            &html[start + block.len()..]
        );
        let a = parse_violation_detail(4242, &fixture("violation_detail.html")).unwrap();
        let b = parse_violation_detail(4242, &html).unwrap();
        assert_eq!(a.subject, b.subject);
        assert_eq!(a.unit, b.unit);
        assert_eq!(a.recipients, b.recipients);
        assert_eq!(a.violation_type, b.violation_type);
        assert_eq!(a.created_by, b.created_by);
        assert_eq!(a.stage, b.stage);
        assert_eq!(a.violation_date, b.violation_date);
        assert_eq!(a.due_date, b.due_date);
        assert!(html.find("Subject:").unwrap() < html.find("Unit Involved").unwrap());
    }

    #[test]
    fn debug_output_never_contains_the_key() {
        let d = parse_violation_detail(4242, &fixture("violation_detail.html")).unwrap();
        let out = format!("{d:?}");
        assert!(!out.contains(FAKE_KEY));
        assert!(out.contains("<redacted>"));
    }

    #[test]
    fn fixtures_carry_no_real_keys() {
        for entry in std::fs::read_dir("tests/fixtures").unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.starts_with("violation") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for part in text.split("Key=").skip(1) {
                let value: String = part
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
                    .collect();
                assert_eq!(value, FAKE_KEY, "{name} carries a non-fake Key");
            }
        }
    }

    #[test]
    fn renamed_update_body_class_is_a_structure_error() {
        let html = fixture("violation_detail.html").replace("divUpdateText", "divRenamed");
        assert!(matches!(
            parse_violation_detail(4242, &html),
            Err(ParseError::MissingElement("td.divUpdateText"))
        ));
    }

    fn with_body(body: &str) -> String {
        let html = fixture("violation_detail_empty.html");
        let table = format!(
            r#"<table><tr><td><i class="fas fa-user"></i><span><span>A</span></span> 03/04/2026 08:05 AM</td><td class="divUpdateText">{body}</td></tr></table>"#
        );
        html.replace(
            r#"<div id="updateRows">"#,
            &format!(r#"<div id="updateRows">{table}"#),
        )
    }

    #[test]
    fn malformed_attachment_urls_never_leak_the_key() {
        let secret = "ABCDEF12-3456-7890-ABCD-EF1234567890";
        let bodies = [
            format!(r#"<p><img src="https://x/access-file.aspx?Key={secret}&mode=preview"></p>"#),
            format!(
                r#"<p><img src="https://x/access-file.aspx?filerecordid=77&key={secret}"></p>"#
            ),
            format!(
                r#"<p><img src="https://x/access-file.aspx?FileRecordID=abc&Key={secret}"></p>"#
            ),
            format!(
                r#"<video><source src="https://x/access-file.aspx?FileRecordID=5&Key={secret}"></video><p>t</p>"#
            ),
            format!(r#"<p><a href="https://x/y?Key={secret}">link</a></p>"#),
        ];
        for body in bodies {
            let d = parse_violation_detail(4242, &with_body(&body)).unwrap();
            let md = &d.updates[0].body_markdown;
            assert!(!md.to_lowercase().contains(&secret.to_lowercase()), "{md}");
            assert!(!format!("{d:?}")
                .to_lowercase()
                .contains(&secret.to_lowercase()));
        }
    }

    #[test]
    fn attachment_labels_never_carry_the_key() {
        let secret = "ABCDEF12-3456-7890-ABCD-EF1234567890";
        let url = format!("https://x/access-file.aspx?FileRecordID=5&Key={secret}");
        let bodies = [
            format!(r#"<p><a href="{url}">{url}</a></p>"#),
            format!(r#"<p><a href="{url}" title="{url}">x</a></p>"#),
            format!(r#"<p><img alt="{url}" src="{url}"></p>"#),
        ];
        for body in bodies {
            let d = parse_violation_detail(4242, &with_body(&body)).unwrap();
            let a = &d.updates[0].attachments[0];
            assert_eq!(a.name, None, "{body}");
            assert!(!format!("{d:?}").contains(secret));
        }
        let ok = format!(r#"<p><a href="{url}">kayak permit</a></p>"#);
        let d = parse_violation_detail(4242, &with_body(&ok)).unwrap();
        assert_eq!(
            d.updates[0].attachments[0].name.as_deref(),
            Some("kayak permit")
        );
    }
}
