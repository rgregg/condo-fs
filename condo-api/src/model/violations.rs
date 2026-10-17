use crate::error::{ClientError, ParseError};
use chrono::NaiveDate;
use scraper::{Html, Selector};
use serde::Deserialize;

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
}
