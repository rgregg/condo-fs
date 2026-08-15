use crate::error::ParseError;
use chrono::NaiveDateTime;
use scraper::{ElementRef, Html, Selector};

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

/// One file attached to an announcement. Listed only — the content is not fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub uploaded: Option<NaiveDateTime>,
}

/// A single announcement with its full text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncementDetail {
    pub id: u64,
    pub title: String,
    pub posted_at: Option<NaiveDateTime>,
    pub expires_at: Option<NaiveDateTime>,
    pub posted_by: Option<String>,
    pub body_markdown: String,
    pub attachments: Vec<Attachment>,
}

/// Read the value that follows a `label.control-label` whose text is `label_text`.
///
/// Matched by label text and then by that specific label's own following siblings, not by
/// pairing the Nth matching label with the Nth `p.form-control-static` in the whole
/// document: the detail page reorders and omits fields depending on how the announcement
/// was created, and a document-wide index would silently misalign the moment any field
/// carrying its own `p.form-control-static` (in the body, in an attachments row, etc.)
/// broke the assumed one-to-one correspondence. Walking forward from the matched label
/// itself keeps each value scoped to its own label regardless of what else is on the page.
fn labelled_value(doc: &Html, label_text: &str) -> Option<String> {
    let label_sel = Selector::parse("label.control-label").unwrap();
    let value_sel = Selector::parse("p.form-control-static").unwrap();

    let label = doc
        .select(&label_sel)
        .find(|l| l.text().collect::<String>().trim() == label_text)?;

    let mut sibling = label.next_sibling();
    while let Some(node) = sibling {
        if let Some(el) = ElementRef::wrap(node) {
            if let Some(value) = el.select(&value_sel).next() {
                return Some(value.text().collect::<String>().trim().to_string());
            }
        }
        sibling = node.next_sibling();
    }
    None
}

/// Find the `table.table-striped` that actually lists attachments, identified by a header
/// containing "File Name" rather than by being first or only such table on the page: the
/// full page can carry other `table.table-striped` widgets, and picking the wrong one
/// would surface garbage rows as attachments instead of an empty (or correct) list.
fn find_attachments_table(doc: &Html) -> Option<ElementRef<'_>> {
    let table_sel = Selector::parse("table.table-striped").unwrap();
    let th_sel = Selector::parse("th").unwrap();
    doc.select(&table_sel).find(|table| {
        table
            .select(&th_sel)
            .any(|th| th.text().collect::<String>().contains("File Name"))
    })
}

/// Parse an announcement detail page.
///
/// `id` is passed in rather than scraped: the caller already knows it, and the page does not
/// display it. Every metadata field is matched by its own label's DOM neighbourhood (see
/// `labelled_value`); only the body is required — a page without `div.ck-content` is a
/// structure error, since an agent must never be told an announcement has no text because
/// the markup moved.
pub fn parse_announcement_detail(id: u64, html: &str) -> Result<AnnouncementDetail, ParseError> {
    let doc = Html::parse_fragment(html);

    let body_sel = Selector::parse("div.ck-content").unwrap();
    let body_el = doc
        .select(&body_sel)
        .next()
        .ok_or(ParseError::MissingElement("div.ck-content"))?;
    let body_markdown = htmd::convert(&body_el.html()).unwrap_or_default();

    let cell_sel = Selector::parse("td").unwrap();
    let attachments = find_attachments_table(&doc)
        .map(|table| {
            let row_sel = Selector::parse("tbody tr").unwrap();
            table
                .select(&row_sel)
                .filter_map(|row| {
                    let cells: Vec<String> = row
                        .select(&cell_sel)
                        .map(|c| c.text().collect::<String>().trim().to_string())
                        .collect();
                    // Columns are Options | File Name | Date Uploaded.
                    let name = cells.get(1)?.clone();
                    if name.is_empty() {
                        return None;
                    }
                    Some(Attachment {
                        name,
                        uploaded: cells.get(2).and_then(|s| parse_us_datetime(s)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(AnnouncementDetail {
        id,
        title: labelled_value(&doc, "Title:").unwrap_or_default(),
        posted_at: labelled_value(&doc, "Posted:").and_then(|s| parse_us_datetime(&s)),
        expires_at: labelled_value(&doc, "Expires:").and_then(|s| parse_us_datetime(&s)),
        posted_by: labelled_value(&doc, "Posted By:"),
        body_markdown,
        attachments,
    })
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

    fn detail_fixture() -> String {
        std::fs::read_to_string("tests/fixtures/announcement_detail.html").unwrap()
    }

    #[test]
    fn parses_the_captured_detail_page() {
        let d = parse_announcement_detail(1541631, &detail_fixture()).unwrap();
        assert_eq!(d.id, 1541631);
        assert_eq!(d.title, "Walk-About Garage Sale - Labor Day Weekend!");
        assert_eq!(d.posted_by.as_deref(), Some("HPMA PRC"));
        assert_eq!(
            d.posted_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(16, 14, 37)
                .unwrap()
        );
        assert_eq!(
            d.expires_at.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap()
        );
        assert_eq!(d.attachments.len(), 1);
        assert_eq!(d.attachments[0].name, "Garage Sale.pdf");
        assert_eq!(
            d.attachments[0].uploaded.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 8, 13)
                .unwrap()
                .and_hms_opt(16, 14, 38)
                .unwrap()
        );
    }

    #[test]
    fn the_body_is_markdown_with_no_html_tags_left() {
        let d = parse_announcement_detail(1541631, &detail_fixture()).unwrap();
        assert!(
            !d.body_markdown.contains("<div") && !d.body_markdown.contains("<figure"),
            "an agent must never receive raw markup: {}",
            d.body_markdown
        );
    }

    #[test]
    fn labels_are_matched_by_text_not_by_position() {
        // Same four labels, reordered and with an extra field inserted. A parser keyed on
        // sibling index would silently mis-assign these.
        let html = r#"<div>
            <label class="control-label">Posted By:</label>
            <div><p class="form-control-static">Someone Else</p></div>
            <label class="control-label">Unrelated:</label>
            <div><p class="form-control-static">ignore me</p></div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Reordered</p></div>
            <div class="ck-content"><p>Body text</p></div>
        </div>"#;
        let d = parse_announcement_detail(7, html).unwrap();
        assert_eq!(d.title, "Reordered");
        assert_eq!(d.posted_by.as_deref(), Some("Someone Else"));
        assert_eq!(d.body_markdown.trim(), "Body text");
    }

    #[test]
    fn a_detail_page_without_a_body_is_an_error() {
        let html = r#"<div><label class="control-label">Title:</label>
            <div><p class="form-control-static">No body</p></div></div>"#;
        assert!(matches!(
            parse_announcement_detail(1, html),
            Err(ParseError::MissingElement("div.ck-content"))
        ));
    }

    #[test]
    fn a_detail_page_with_no_attachments_table_yields_an_empty_vec() {
        let html = r#"<div><label class="control-label">Title:</label>
            <div><p class="form-control-static">No attachments</p></div>
            <div class="ck-content"><p>Body</p></div></div>"#;
        let d = parse_announcement_detail(1, html).unwrap();
        assert!(d.attachments.is_empty());
    }

    #[test]
    fn a_decoy_table_striped_does_not_leak_garbage_attachment_rows() {
        // Mirrors the list-page lesson: a full page can carry more than one
        // table.table-striped (shell widgets, other content blocks). Blindly matching
        // "table.table-striped tbody tr" across the whole document would pick up rows
        // from a decoy table that carry no file at all.
        let html = r#"<div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Has decoy</p></div>
            <div class="ck-content"><p>Body</p></div>
            <table class="table table-striped">
                <thead><tr><th>Unrelated</th><th>Widget</th></tr></thead>
                <tbody><tr><td>not</td><td>an attachment</td></tr></tbody>
            </table>
            <table class="table table-striped">
                <thead><tr><th>Options</th><th>File Name</th><th>Date Uploaded</th></tr></thead>
                <tbody><tr>
                    <td><a href="/download">Download</a></td>
                    <td>Real File.pdf</td>
                    <td>1/2/2026 9:05:00 AM</td>
                </tr></tbody>
            </table>
        </div>"#;
        let d = parse_announcement_detail(1, html).unwrap();
        assert_eq!(
            d.attachments.len(),
            1,
            "must not leak the decoy table's row"
        );
        assert_eq!(d.attachments[0].name, "Real File.pdf");
    }
}
