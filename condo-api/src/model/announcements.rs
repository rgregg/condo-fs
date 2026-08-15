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

/// The outcome of scanning a node's following siblings for a field's value.
///
/// Kept as three states rather than collapsing to `Option<String>` because
/// `labelled_value`'s stage-2 fallback must react differently to each of the two
/// "not found" cases: hitting the next label means this field genuinely has no value
/// here, so the fallback must stop; running out of siblings means the search simply
/// hasn't reached the value yet, so the fallback may keep looking further out. A plain
/// `None` cannot distinguish those, which is exactly how the fallback used to walk past
/// a label with no value of its own and return the *next* field's value instead.
enum SiblingScan {
    Found(String),
    HitNextLabel,
    Exhausted,
}

/// Search `from`'s own following siblings for a value, stopping as soon as another
/// `label.control-label` is reached (whether the sibling itself is one, or contains one).
///
/// The "stop at the next label" rule is what keeps this scoped to a single field: without
/// it, a label with no value of its own would silently return whatever value belongs to
/// the *next* field instead of `None` — wrong data standing in for a missing one, which is
/// exactly the class of bug this whole matching scheme exists to avoid.
fn value_in_following_siblings(
    from: ElementRef<'_>,
    label_sel: &Selector,
    value_sel: &Selector,
) -> SiblingScan {
    let mut sibling = from.next_sibling();
    while let Some(node) = sibling {
        if let Some(el) = ElementRef::wrap(node) {
            if label_sel.matches(&el) || el.select(label_sel).next().is_some() {
                return SiblingScan::HitNextLabel; // ran into the next field first
            }
            if let Some(value) = el.select(value_sel).next() {
                return SiblingScan::Found(value.text().collect::<String>().trim().to_string());
            }
        }
        sibling = node.next_sibling();
    }
    SiblingScan::Exhausted
}

/// Read the value that follows a `label.control-label` whose text is `label_text`.
///
/// Matched by label text and then by that specific label's own local DOM neighbourhood,
/// not by pairing the Nth matching label with the Nth `p.form-control-static` in the whole
/// document: the detail page reorders and omits fields depending on how the announcement
/// was created, and a document-wide index would silently misalign the moment any field
/// carrying its own `p.form-control-static` (in the body, in an attachments row, etc.)
/// broke the assumed one-to-one correspondence.
///
/// Two things are tried, in order:
/// 1. The label's own following siblings (the common case: label and value share a
///    wrapper, e.g. `<div><label>Title:</label><div><p class="form-control-static">…`).
/// 2. If the label itself has no qualifying sibling — Bootstrap sometimes wraps the label
///    in its own column `<div>`, e.g. `<div class="col-md-4"><label>Title:</label></div>`
///    — the same search from the label's *enclosing block's* following siblings.
///
/// Stage 2 only runs when stage 1 is `Exhausted` (ran out of siblings without finding
/// either a value or the next label). If stage 1 instead hits the next label, that means
/// this label has no value of its own *and* it's not merely wrapped alone — falling
/// through to the enclosing block in that case would walk out to the next field's value
/// and return it here, exactly the bug this whole scheme exists to avoid.
///
/// A value element that resolves to only whitespace is treated the same as no value
/// element at all: `None` means "not stated", and `Some("")` is never a valid answer.
fn labelled_value(doc: &Html, label_text: &str) -> Option<String> {
    let label_sel = Selector::parse("label.control-label").unwrap();
    let value_sel = Selector::parse("p.form-control-static").unwrap();

    let label = doc
        .select(&label_sel)
        .find(|l| l.text().collect::<String>().trim() == label_text)?;

    let scan = match value_in_following_siblings(label, &label_sel, &value_sel) {
        SiblingScan::Found(v) => SiblingScan::Found(v),
        SiblingScan::HitNextLabel => SiblingScan::HitNextLabel,
        SiblingScan::Exhausted => {
            let enclosing_block = ElementRef::wrap(label.parent()?)?;
            value_in_following_siblings(enclosing_block, &label_sel, &value_sel)
        }
    };

    match scan {
        SiblingScan::Found(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Normalize header text for comparison: lowercase and collapse internal whitespace to
/// single spaces, so a re-cased or internally-respaced header (e.g. "File name",
/// "File   Name") is still recognized instead of silently failing closed to an empty
/// attachments list.
fn normalize_header(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Find the `table.table-striped` that actually lists attachments, identified by a header
/// containing "File Name" rather than by being first or only such table on the page: the
/// full page can carry other `table.table-striped` widgets, and picking the wrong one
/// would surface garbage rows as attachments instead of an empty (or correct) list.
///
/// Returns the table along with the column indices for the name and upload-date cells,
/// read from that same header row rather than assumed as fixed positions: if Condo
/// Control ever drops or reorders a column (e.g. removing "Options"), a hardcoded index
/// would silently read the wrong text into `name` instead of failing loudly or matching
/// correctly.
fn find_attachments_table(doc: &Html) -> Option<(ElementRef<'_>, usize, Option<usize>)> {
    let table_sel = Selector::parse("table.table-striped").unwrap();
    let row_sel = Selector::parse("tr").unwrap();
    let th_sel = Selector::parse("th").unwrap();
    doc.select(&table_sel).find_map(|table| {
        // Scoped to the one row whose own `th`s actually label the data columns (found
        // by content, same discipline as everywhere else in this module) — not every
        // `th` in the table. A second thead row above it (a colspan'd grouping header)
        // or a `th` stranded in tbody would otherwise add extra entries and shift the
        // derived column indices off the row's actual `td` columns.
        let header_row = table.select(&row_sel).find(|r| {
            r.select(&th_sel)
                .any(|th| normalize_header(&th.text().collect::<String>()).contains("file name"))
        })?;
        let headers: Vec<String> = header_row
            .select(&th_sel)
            .map(|th| normalize_header(&th.text().collect::<String>()))
            .collect();
        let name_idx = headers.iter().position(|h| h.contains("file name"))?;
        let uploaded_idx = headers.iter().position(|h| h.contains("date uploaded"));
        Some((table, name_idx, uploaded_idx))
    })
}

/// Parse an announcement detail page.
///
/// `id` is passed in rather than scraped: the caller already knows it, and the page does not
/// display it. Every metadata field is matched by its own label's DOM neighbourhood (see
/// `labelled_value`). The title and the body are both required: a page without a `Title:`
/// label, or without `div.ck-content`, is a structure error rather than an announcement
/// with an empty title or empty text — an agent must never be told an announcement has no
/// title or no text because the markup moved.
pub fn parse_announcement_detail(id: u64, html: &str) -> Result<AnnouncementDetail, ParseError> {
    let doc = Html::parse_fragment(html);

    let title = labelled_value(&doc, "Title:")
        .ok_or(ParseError::MissingElement("label.control-label:Title:"))?;

    let body_sel = Selector::parse("div.ck-content").unwrap();
    let body_el = doc
        .select(&body_sel)
        .next()
        .ok_or(ParseError::MissingElement("div.ck-content"))?;
    let body_markdown = htmd::convert(&body_el.html())
        .map_err(|_| ParseError::UnconvertibleMarkup("div.ck-content"))?;

    let cell_sel = Selector::parse("td").unwrap();
    let attachments = find_attachments_table(&doc)
        .map(|(table, name_idx, uploaded_idx)| {
            let row_sel = Selector::parse("tbody tr").unwrap();
            table
                .select(&row_sel)
                .filter_map(|row| {
                    let cells: Vec<String> = row
                        .select(&cell_sel)
                        .map(|c| c.text().collect::<String>().trim().to_string())
                        .collect();
                    let name = cells.get(name_idx)?.clone();
                    if name.is_empty() {
                        return None;
                    }
                    Some(Attachment {
                        name,
                        uploaded: uploaded_idx
                            .and_then(|i| cells.get(i))
                            .and_then(|s| parse_us_datetime(s)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(AnnouncementDetail {
        id,
        title,
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
        // `the_body_is_markdown_with_no_html_tags_left` and
        // `!body_md.contains("<div")` alone are satisfied by an empty string — pin the
        // real content too, so a conversion that silently produces nothing cannot pass.
        assert!(
            !d.body_markdown.trim().is_empty(),
            "the captured fixture's body is a real image link, not empty text"
        );
        assert!(
            d.body_markdown.contains("access-file.aspx"),
            "expected the captured image URL to survive conversion, got: {}",
            d.body_markdown
        );
    }

    #[test]
    fn a_label_with_no_value_of_its_own_does_not_steal_the_next_fields_value() {
        // Both labels share a sibling-bearing enclosing div, so stage 1 (the labels'
        // own following siblings, which finds nothing for either label) correctly falls
        // through to stage 2 (the enclosing block's following siblings) — but stage 2
        // must stop there too. A stage-2 fallback that doesn't distinguish "hit the next
        // label" from "ran out of siblings" would keep walking out to the *enclosing
        // block's* following siblings and wrongly return "Posted By:"'s value for
        // "Title:" instead of None. Tested directly against `labelled_value` (a private
        // fn, reachable from this child module) so the assertion is about the matching
        // logic itself, not laundered through the higher-level parse result.
        let html = r#"<div>
            <div class="col-lg-6">
                <label class="control-label">Title:</label>
                <label class="control-label">Posted By:</label>
            </div>
            <div class="col-lg-6"><p class="form-control-static">HPMA PRC</p></div>
        </div>"#;
        let doc = Html::parse_fragment(html);
        assert_eq!(
            labelled_value(&doc, "Title:"),
            None,
            "a label with no value of its own must not report the next field's value"
        );
        assert_eq!(
            labelled_value(&doc, "Posted By:").as_deref(),
            Some("HPMA PRC")
        );
    }

    #[test]
    fn a_blank_value_element_is_none_not_an_empty_string() {
        // `Some("")` is a wrong API shape regardless of the stage-1/stage-2 fallback
        // question: None means "not stated", Some("") means nothing and must never be
        // handed to a caller. A value element that exists but is only whitespace must
        // read the same as no value element at all.
        let html = r#"<div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">   </p></div>
        </div>"#;
        let doc = Html::parse_fragment(html);
        assert_eq!(
            labelled_value(&doc, "Title:"),
            None,
            "a blank value element must read as None, not Some(\"\")"
        );
    }

    #[test]
    fn a_detail_page_with_a_blank_title_value_is_an_error() {
        // The doc comment on parse_announcement_detail promises a page without a usable
        // Title: is a structure error, not an announcement with an empty title.
        let html = r#"<div><label class="control-label">Title:</label>
            <div><p class="form-control-static">   </p></div>
            <div class="ck-content"><p>Body</p></div></div>"#;
        assert!(matches!(
            parse_announcement_detail(1, html),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn a_stray_value_before_the_labels_is_not_picked_up() {
        // An index-paired implementation (label N <-> value N in document order) would
        // hand this stray paragraph to the first label it sees, since it's the first
        // `p.form-control-static` in the document. Matching from the label's own
        // neighbourhood must skip it.
        let html = r#"<div>
            <p class="form-control-static">Stray value with no label of its own</p>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Real Title</p></div>
        </div>"#;
        let doc = Html::parse_fragment(html);
        assert_eq!(
            labelled_value(&doc, "Title:").as_deref(),
            Some("Real Title")
        );
    }

    #[test]
    fn a_label_wrapped_in_its_own_column_div_still_finds_its_value() {
        // Bootstrap sometimes wraps the label itself in a column div, leaving it with no
        // sibling of its own inside that div. The value must still be found by falling
        // back to the label's enclosing block's following siblings.
        let html = r#"<div>
            <div class="col-md-4"><label class="control-label">Title:</label></div>
            <div class="col-md-8"><p class="form-control-static">Wrapped Title</p></div>
        </div>"#;
        let doc = Html::parse_fragment(html);
        assert_eq!(
            labelled_value(&doc, "Title:").as_deref(),
            Some("Wrapped Title")
        );
    }

    #[test]
    fn a_detail_page_without_a_title_is_an_error() {
        // The plan treats a missing body as a structure error rather than an empty
        // string; a missing title gets the same treatment, for the same reason — an
        // agent must never be told an announcement has an empty title because the
        // markup moved.
        let html = r#"<div><div class="ck-content"><p>Body with no title label</p></div></div>"#;
        assert!(matches!(
            parse_announcement_detail(1, html),
            Err(ParseError::MissingElement(_))
        ));
    }

    #[test]
    fn attachment_columns_are_found_by_header_text_not_fixed_position() {
        // Same discipline as the table-selection tests: if the "Options" column were
        // ever dropped, cells.get(1)/get(2) would silently read the date into `name`
        // and the filename into `uploaded`. Reordering the columns here must not
        // confuse name and date.
        let html = r#"<div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Reordered columns</p></div>
            <div class="ck-content"><p>Body</p></div>
            <table class="table table-striped">
                <thead><tr><th>Date Uploaded</th><th>File Name</th></tr></thead>
                <tbody><tr>
                    <td>1/2/2026 9:05:00 AM</td>
                    <td>Reordered.pdf</td>
                </tr></tbody>
            </table>
        </div>"#;
        let d = parse_announcement_detail(1, html).unwrap();
        assert_eq!(d.attachments.len(), 1);
        assert_eq!(d.attachments[0].name, "Reordered.pdf");
        assert_eq!(
            d.attachments[0].uploaded.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2)
                .unwrap()
                .and_hms_opt(9, 5, 0)
                .unwrap()
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
    fn attachment_headers_are_matched_regardless_of_case_and_whitespace() {
        // A re-cased or internally-respaced header (e.g. "File name", "File   Name")
        // must still be recognized: brittle exact-case, exact-spacing matching would
        // silently fail closed to an empty attachments list instead of finding the
        // real table.
        let html = r#"<div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Recased headers</p></div>
            <div class="ck-content"><p>Body</p></div>
            <table class="table table-striped">
                <thead><tr><th>File   name</th><th>DATE UPLOADED</th></tr></thead>
                <tbody><tr>
                    <td>Recased.pdf</td>
                    <td>1/2/2026 9:05:00 AM</td>
                </tr></tbody>
            </table>
        </div>"#;
        let d = parse_announcement_detail(1, html).unwrap();
        assert_eq!(d.attachments.len(), 1);
        assert_eq!(d.attachments[0].name, "Recased.pdf");
        assert_eq!(
            d.attachments[0].uploaded.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2)
                .unwrap()
                .and_hms_opt(9, 5, 0)
                .unwrap()
        );
    }

    #[test]
    fn a_grouping_header_row_does_not_shift_column_indices() {
        // find_attachments_table must scope its header scan to the header row that
        // actually labels the data columns. A two-row thead (a grouping row above the
        // real column-header row) adds th's that don't correspond 1:1 with tbody's td
        // columns; collecting th across the whole table would count that grouping th
        // and derive name/uploaded indices one column too far right, reading the wrong
        // cell into `name`.
        let html = r#"<div>
            <label class="control-label">Title:</label>
            <div><p class="form-control-static">Grouped header</p></div>
            <div class="ck-content"><p>Body</p></div>
            <table class="table table-striped">
                <thead>
                    <tr><th colspan="3">Attachment</th></tr>
                    <tr><th>Options</th><th>File Name</th><th>Date Uploaded</th></tr>
                </thead>
                <tbody><tr>
                    <td><a href="/download">Download</a></td>
                    <td>Real.pdf</td>
                    <td>1/2/2026 9:05:00 AM</td>
                </tr></tbody>
            </table>
        </div>"#;
        let d = parse_announcement_detail(1, html).unwrap();
        assert_eq!(d.attachments.len(), 1);
        assert_eq!(d.attachments[0].name, "Real.pdf");
        assert_eq!(
            d.attachments[0].uploaded.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2)
                .unwrap()
                .and_hms_opt(9, 5, 0)
                .unwrap()
        );
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
