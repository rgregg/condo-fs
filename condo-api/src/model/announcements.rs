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
/// Naive local time — the page carries no zone. `%l` (space-padded 12-hour) is used
/// rather than `%I` (zero-padded): chrono's parser accepts either a padded or
/// unpadded hour regardless of which specifier is used, so a second format string as
/// a fallback would never actually be reached — one is enough.
pub(crate) fn parse_us_datetime(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim(), "%m/%d/%Y %l:%M:%S %p").ok()
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

    // A page can legitimately carry more than one `table.table-striped` — the detail
    // page has one for attachments, and a shell widget could add one to this page too.
    // Prefer the table that actually holds announcement links; fall back to the first
    // `table.table-striped` only when none does, so a genuinely empty content table
    // still reads as an empty list rather than a structure error.
    let table = doc
        .select(&table_sel)
        .find(|t| t.select(&link_sel).next().is_some())
        .or_else(|| doc.select(&table_sel).next())
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
            log::warn!("skipping an announcement row: unreadable id in link href");
            continue;
        };
        let title = link.text().collect::<String>().trim().to_string();

        let spans: Vec<String> = row
            .select(&span_sel)
            .map(|s| s.text().collect::<String>().trim().to_string())
            .collect();
        // The posted timestamp is whichever non-"Expires:" span parses as a date, not
        // simply the first span in document order: the title cell's preview text or a
        // reordered "Expires:" span must not be mistaken for it.
        let Some(posted_at) = spans
            .iter()
            .filter(|s| !s.starts_with("Expires:"))
            .find_map(|s| parse_us_datetime(s))
        else {
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

    #[test]
    fn a_decoy_span_before_the_date_does_not_hide_it() {
        // The title cell can carry a bare-text preview today, but if the shell ever
        // wraps that preview in its own <span>, a parser that trusts document order
        // would read the preview as the posted date and drop the row. The date must be
        // found by content (it's the span that parses as a date), not by position.
        let html = r#"<table class="table table-striped"><tbody><tr>
            <td><strong><a class="announcementTitle" href="/announcement/detail/42">Title</a></strong>
                <span>Not a date, just a preview blurb</span></td>
            <td><span>1/2/2026 9:05:00 AM</span></td>
        </tr></tbody></table>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(items.len(), 1, "the decoy span must not sink the whole row");
        assert_eq!(items[0].id, 42);
        assert_eq!(
            items[0].posted_at,
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2)
                .unwrap()
                .and_hms_opt(9, 5, 0)
                .unwrap()
        );
    }

    #[test]
    fn an_expiry_span_appearing_first_is_not_mistaken_for_the_posted_date() {
        // Reordering the two date spans is the same class of bug as the decoy-span
        // case above: the posted date must be identified by not being an "Expires:"
        // span, not by being first.
        let html = r#"<table class="table table-striped"><tbody><tr>
            <td><strong><a class="announcementTitle" href="/announcement/detail/42">Title</a></strong></td>
            <td><span>Expires: 10/1/2026 11:59:59 PM</span>
                <span>1/2/2026 9:05:00 AM</span></td>
        </tr></tbody></table>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].posted_at,
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2)
                .unwrap()
                .and_hms_opt(9, 5, 0)
                .unwrap()
        );
        assert_eq!(
            items[0].expires_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap()
        );
    }

    #[test]
    fn a_decoy_table_striped_before_the_content_table_is_skipped() {
        // The real page can carry more than one table.table-striped (the detail page's
        // attachments table is one example). A parser keyed on "the first
        // table.table-striped" would bind a shell widget's table here and report zero
        // announcements instead of erroring or finding the real content.
        let html = r#"<div>
            <table class="table table-striped"><tbody><tr><td>unrelated widget</td></tr></tbody></table>
            <table class="table table-striped"><tbody><tr>
                <td><strong><a class="announcementTitle" href="/announcement/detail/42">Title</a></strong></td>
                <td><span>1/2/2026 9:05:00 AM</span></td>
            </tr></tbody></table>
        </div>"#;
        let items = parse_announcement_list(html).unwrap();
        assert_eq!(
            items.len(),
            1,
            "must find the real table, not bind the decoy"
        );
        assert_eq!(items[0].id, 42);
    }
}
