//! Label/value and HTML-to-Markdown helpers shared by the page parsers.

use crate::error::ParseError;
use scraper::{ElementRef, Html, Selector};

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
            // The sibling may itself be the value element (violations) or wrap it
            // (announcements).
            let value = if value_sel.matches(&el) {
                Some(el)
            } else {
                el.select(value_sel).next()
            };
            if let Some(value) = value {
                return SiblingScan::Found(value.text().collect::<String>().trim().to_string());
            }
        }
        sibling = node.next_sibling();
    }
    SiblingScan::Exhausted
}

/// Read the value that follows a `label.control-label` whose text is `label_text`; the
/// value element is whatever `value_css` matches (a `p.form-control-static` on
/// announcements, a `span.textLeft` on violations).
///
/// Matched by label text and then by that specific label's own local DOM neighbourhood,
/// not by pairing the Nth matching label with the Nth value element in the whole
/// document: the detail page reorders and omits fields depending on how the announcement
/// was created, and a document-wide index would silently misalign the moment any field
/// carrying its own value element (in the body, in an attachments row, etc.)
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
pub(crate) fn labelled_value(doc: &Html, label_text: &str, value_css: &str) -> Option<String> {
    let label_sel = Selector::parse("label.control-label").unwrap();
    let value_sel = Selector::parse(value_css).unwrap();

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

/// Convert an HTML fragment to Markdown. `what` names the element for the error.
pub(crate) fn to_markdown(html: &str, what: &'static str) -> Result<String, ParseError> {
    htmd::convert(html).map_err(|_| ParseError::UnconvertibleMarkup(what))
}
