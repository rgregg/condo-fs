use crate::error::ParseError;
use chrono::NaiveDateTime;
use scraper::{Html, Selector};

/// One row of the announcements list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncementSummary {
    pub id: u64,
    pub title: String,
    pub posted_at: NaiveDateTime,
    /// Absent when the announcement has no expiry.
    pub expires_at: Option<NaiveDateTime>,
}

/// Parse Condo Control's US-format local timestamps, e.g. `8/13/2026 4:14:37 PM`.
///
/// Naive local time — the page carries no zone.
pub(crate) fn parse_us_datetime(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%Y %l:%M:%S %p")
        .or_else(|_| NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%Y %I:%M:%S %p"))
        .ok()
}

/// Parse the announcements list page.
///
/// Selectors are anchored on class names because the surrounding page is a full application
/// shell that changes independently of this content. A missing table is an error rather than
/// an empty result: an agent must never be told there are no announcements because the markup
/// moved.
pub fn parse_announcement_list(html: &str) -> Result<Vec<AnnouncementSummary>, ParseError> {
    let doc = Html::parse_fragment(html);
    let table_sel = Selector::parse("table.table-striped").unwrap();
    let row_sel = Selector::parse("tbody tr").unwrap();
    let link_sel = Selector::parse("a.announcementTitle").unwrap();
    let span_sel = Selector::parse("span").unwrap();

    let table = doc
        .select(&table_sel)
        .next()
        .ok_or(ParseError::MissingElement("table.table-striped"))?;

    let mut out = Vec::new();
    for row in table.select(&row_sel) {
        let Some(link) = row.select(&link_sel).next() else {
            continue; // header/footer rows carry no announcement link
        };
        let Some(id) = link
            .value()
            .attr("href")
            .and_then(|h| h.rsplit('/').next())
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        let title = link.text().collect::<String>().trim().to_string();

        let spans: Vec<String> = row
            .select(&span_sel)
            .map(|s| s.text().collect::<String>().trim().to_string())
            .collect();
        // First span is the posted timestamp; an "Expires: …" span may follow.
        let Some(posted_at) = spans.first().and_then(|s| parse_us_datetime(s)) else {
            log::warn!("skipping announcement {id}: unreadable posted date");
            continue;
        };
        let expires_at = spans
            .iter()
            .find_map(|s| s.strip_prefix("Expires:"))
            .and_then(parse_us_datetime);

        out.push(AnnouncementSummary {
            id,
            title,
            posted_at,
            expires_at,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.posted_at));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string("tests/fixtures/announcements_list.html").unwrap()
    }

    #[test]
    fn parses_every_row_of_the_captured_list() {
        let items = parse_announcement_list(&fixture()).unwrap();
        assert_eq!(items.len(), 3);
        let first = &items[0];
        assert_eq!(first.id, 1541631);
        assert_eq!(first.title, "Walk-About Garage Sale - Labor Day Weekend!");
        assert_eq!(
            first.posted_at,
            chrono::NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(16, 14, 37)
                .unwrap(),
            "4:14:37 PM must parse as 16:14:37, not 04:14:37"
        );
        assert_eq!(
            first.expires_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap()
        );
    }

    #[test]
    fn newest_first() {
        let items = parse_announcement_list(&fixture()).unwrap();
        let mut sorted = items.clone();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.posted_at));
        assert_eq!(
            items, sorted,
            "callers asking 'what's new' depend on this order"
        );
    }

    #[test]
    fn an_empty_table_is_an_empty_list_not_an_error() {
        let html = r#"<div><table class="table table-striped">
            <thead><tr><th>Title</th></tr></thead><tbody></tbody></table></div>"#;
        assert_eq!(parse_announcement_list(html).unwrap().len(), 0);
    }

    #[test]
    fn a_missing_table_is_an_error_not_an_empty_list() {
        // If the markup moves, an agent must not be told there are no announcements.
        let html = "<html><body><p>Something else entirely</p></body></html>";
        assert!(matches!(
            parse_announcement_list(html),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn a_row_without_an_expiry_parses_with_none() {
        let html = r#"<table class="table table-striped"><tbody><tr>
            <td><strong><a class="announcementTitle" href="/announcement/detail/42">No expiry</a></strong></td>
            <td><span>1/2/2026 9:05:00 AM</span></td>
        </tr></tbody></table>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 42);
        assert!(items[0].expires_at.is_none());
    }

    #[test]
    fn a_row_with_an_unreadable_date_is_skipped_not_fatal() {
        // One malformed row must not cost the caller every other announcement.
        let html = r#"<table class="table table-striped"><tbody>
            <tr><td><strong><a class="announcementTitle" href="/announcement/detail/1">Bad</a></strong></td>
                <td><span>not a date</span></td></tr>
            <tr><td><strong><a class="announcementTitle" href="/announcement/detail/2">Good</a></strong></td>
                <td><span>1/2/2026 9:05:00 AM</span></td></tr>
        </tbody></table>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 2);
    }
}
