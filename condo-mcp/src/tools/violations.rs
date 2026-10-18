use crate::server::CondoMcp;
use crate::tools::format_timestamp;
use chrono::NaiveDate;
use condo_api::{
    SavedAttachment, ViolationAttachment, ViolationDetail, ViolationFilterOptions, ViolationQuery,
    ViolationStatus, ViolationSummary, ViolationUpdate, ViolationsApi,
};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

/// Rows per request when scanning for `updated_since`.
const PAGE_SIZE: u32 = 50;
/// At most this many pages (500 rows) are scanned for `updated_since`.
const MAX_SCAN_PAGES: u32 = 10;
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;
/// Timeline entries shown by `get_violation`, newest first.
const TIMELINE_CAP: usize = 25;
/// Upper bound on a snippet, in bytes, ellipsis included.
const SNIPPET_MAX: usize = 160;
/// Downloads larger than this are refused.
const MAX_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;

/// Stages on which the board is moving toward, or has reached, a fine.
const FINE_TRACK_STAGES: [&str; 4] = [
    "Proposed Fine/Option for Appeal Hearing",
    "Owner Declines Hearing",
    "Appeal/Hearing Decision",
    "Fine Issued or Recinded",
];

const FINE_TRACK_NOTE: &str = "This violation is on a fine-track stage. Condo Control does not \
     record stage-change dates, so the dates of earlier notices and of any fine notice are not \
     recorded as such — read the timeline (each update's own timestamp and text) for them.";

const STATUSES: [(&str, ViolationStatus); 7] = [
    ("open", ViolationStatus::Open),
    ("closed", ViolationStatus::Closed),
    ("draft", ViolationStatus::Draft),
    ("past_due", ViolationStatus::PastDue),
    ("void", ViolationStatus::Void),
    ("resident_updated", ViolationStatus::ResidentUpdated),
    ("all", ViolationStatus::All),
];

fn format_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Turn an update's Markdown into one line of plain text of at most `SNIPPET_MAX` bytes.
///
/// Links and images keep only their text, emphasis/heading/quote/list markers and escapes
/// are dropped, and all whitespace collapses to single spaces. A cut lands on a char
/// boundary and is marked with `…`.
pub(crate) fn snippet(markdown: &str) -> String {
    let mut text = String::with_capacity(markdown.len());
    let mut chars = markdown.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(&next) = chars.peek() {
                    if next.is_ascii_punctuation() {
                        text.push(next);
                        chars.next();
                        continue;
                    }
                }
                text.push(c);
            }
            // `![alt](url)` and `[text](url)`: keep the bracketed text, drop the target.
            '!' if chars.peek() == Some(&'[') => {}
            '[' | ']' => {
                if c == ']' && chars.peek() == Some(&'(') {
                    // Skip the `(url)` part.
                    let mut depth = 0;
                    for d in chars.by_ref() {
                        match d {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            '*' | '`' => {}
            '_' if !text.ends_with(|p: char| p.is_alphanumeric())
                || !chars.peek().is_some_and(|n| n.is_alphanumeric()) => {}
            _ => text.push(c),
        }
    }
    let words: Vec<&str> = text
        .lines()
        .map(|line| {
            let t = line.trim_start();
            let t = t
                .trim_start_matches('#')
                .trim_start_matches('>')
                .trim_start();
            match t.strip_prefix("- ").or_else(|| t.strip_prefix("+ ")) {
                Some(rest) => rest,
                None => t,
            }
        })
        .flat_map(str::split_whitespace)
        .collect();
    let joined = words.join(" ");
    if joined.len() <= SNIPPET_MAX {
        return joined;
    }
    let mut cut = SNIPPET_MAX - '…'.len_utf8();
    while !joined.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", joined[..cut].trim_end())
}

/// The `update` value tools exchange: the site's own id when it has one, else the ordinal.
fn update_ref(u: &ViolationUpdate) -> String {
    u.site_id.clone().unwrap_or_else(|| u.ordinal.to_string())
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Every id whose label matches `name` (labels repeat across ids), or an error listing the
/// valid names.
fn resolve_names(options: &[(u64, String)], name: &str, what: &str) -> Result<Vec<u64>, ErrorData> {
    let ids: Vec<u64> = options
        .iter()
        .filter(|(_, label)| same_name(label, name))
        .map(|(id, _)| *id)
        .collect();
    if !ids.is_empty() {
        return Ok(ids);
    }
    let mut names: Vec<&str> = Vec::new();
    for (_, label) in options {
        if !names.iter().any(|n| same_name(n, label)) {
            names.push(label.trim());
        }
    }
    Err(ErrorData::invalid_params(
        format!(
            "Unknown {what} {name:?}. Valid {what} names: {}.",
            names.join("; ")
        ),
        None,
    ))
}

fn parse_status(s: Option<&str>) -> Result<ViolationStatus, ErrorData> {
    let Some(s) = s else {
        return Ok(ViolationStatus::Open);
    };
    STATUSES
        .iter()
        .find(|(name, _)| same_name(name, s))
        .map(|(_, st)| *st)
        .ok_or_else(|| {
            let names: Vec<&str> = STATUSES.iter().map(|(n, _)| *n).collect();
            ErrorData::invalid_params(
                format!("Unknown status {s:?}. Valid: {}.", names.join(", ")),
                None,
            )
        })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListViolationsParams {
    /// One of open, closed, draft, past_due, void, resident_updated, all. Default open.
    pub status: Option<String>,
    /// Free-text search, as the site's own search box.
    pub search: Option<String>,
    /// A stage name, e.g. "30 Day Warning" (case-insensitive).
    pub stage: Option<String>,
    /// A violation type name, e.g. "Article IV Land Use Restrictions" (case-insensitive).
    pub violation_type: Option<String>,
    /// YYYY-MM-DD. Keep only violations last updated on or after this date.
    pub updated_since: Option<String>,
    /// Maximum rows returned. Default 50, max 200.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ViolationRow {
    pub id: u64,
    pub name: String,
    pub violation_type: String,
    pub unit: String,
    pub stage: String,
    /// YYYY-MM-DD.
    pub due_date: Option<String>,
    /// YYYY-MM-DD; the list carries a date only, no time.
    pub last_updated: Option<String>,
    /// The user name the site shows next to the last-updated date.
    pub last_updated_by: Option<String>,
}

impl From<ViolationSummary> for ViolationRow {
    fn from(v: ViolationSummary) -> ViolationRow {
        ViolationRow {
            id: v.id,
            name: v.name,
            violation_type: v.violation_type,
            unit: v.unit,
            stage: v.stage,
            due_date: v.due_date.map(format_date),
            last_updated: v.last_updated.map(format_date),
            last_updated_by: v.last_updated_by,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ViolationList {
    /// The site's total for the status/search/stage/type filter (before updated_since).
    pub total: u64,
    /// Rows that met every filter, the updated_since date included, within the rows
    /// scanned, before `limit` was applied. Without updated_since it equals `returned`.
    pub matched: usize,
    /// Rows in `violations`. Less than `matched` means `limit` cut the list.
    pub returned: usize,
    /// False only when an updated_since scan stopped at its 500-row cap before reaching
    /// older rows: there may be more matches than were seen.
    pub scan_complete: bool,
    pub violations: Vec<ViolationRow>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetViolationParams {
    /// Violation id, as returned by list_violations.
    pub id: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TimelineEntry {
    /// Pass to get_violation_update. Today this is the update's number, 1 = oldest.
    pub update: String,
    /// Local time, no zone.
    pub posted_at: Option<String>,
    pub author: String,
    /// The update's opening text, plain, about 160 characters.
    pub snippet: String,
}

impl From<&ViolationUpdate> for TimelineEntry {
    fn from(u: &ViolationUpdate) -> TimelineEntry {
        TimelineEntry {
            update: update_ref(u),
            posted_at: u.posted_at.map(format_timestamp),
            author: u.author.clone(),
            snippet: snippet(&u.body_markdown),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ViolationSummaryOut {
    pub id: u64,
    pub subject: String,
    pub unit: Option<String>,
    pub recipients: Option<String>,
    pub violation_type: Option<String>,
    pub created_by: Option<String>,
    /// The current stage only; the site keeps no history of stage changes.
    pub stage: Option<String>,
    /// The site's recorded Violation Date field. Local time, no zone.
    pub violation_date: Option<String>,
    /// Local time, no zone.
    pub due_date: Option<String>,
    /// All updates on the violation.
    pub update_count: usize,
    /// Entries in `timeline`; fewer than update_count when the oldest were left out.
    pub timeline_shown: usize,
    /// Newest first, at most 25.
    pub timeline: Vec<TimelineEntry>,
    pub note: Option<String>,
}

impl From<ViolationDetail> for ViolationSummaryOut {
    fn from(d: ViolationDetail) -> ViolationSummaryOut {
        let timeline: Vec<TimelineEntry> = d
            .updates
            .iter()
            .rev()
            .take(TIMELINE_CAP)
            .map(TimelineEntry::from)
            .collect();
        let on_fine_track = d
            .stage
            .as_deref()
            .is_some_and(|s| FINE_TRACK_STAGES.iter().any(|f| same_name(f, s)));
        ViolationSummaryOut {
            id: d.id,
            subject: d.subject,
            unit: d.unit,
            recipients: d.recipients,
            violation_type: d.violation_type,
            created_by: d.created_by,
            stage: d.stage,
            violation_date: d.violation_date.map(format_timestamp),
            due_date: d.due_date.map(format_timestamp),
            update_count: d.updates.len(),
            timeline_shown: timeline.len(),
            timeline,
            note: on_fine_track.then(|| FINE_TRACK_NOTE.to_string()),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetViolationUpdateParams {
    /// Violation id.
    pub id: u64,
    /// The `update` value from get_violation's timeline.
    pub update: String,
    /// The update_count get_violation reported. If the violation has gained updates since,
    /// the call fails instead of returning a different update than intended.
    pub expected_update_count: Option<usize>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AttachmentRef {
    /// Pass to download_violation_attachment.
    pub attachment_id: u64,
    /// A display label from the page, often without a file extension; may be absent.
    pub name: Option<String>,
}

impl From<ViolationAttachment> for AttachmentRef {
    fn from(a: ViolationAttachment) -> AttachmentRef {
        AttachmentRef {
            attachment_id: a.id,
            name: a.name,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ViolationUpdateOut {
    pub update: String,
    /// Local time, no zone.
    pub posted_at: Option<String>,
    pub author: String,
    /// The full update, converted from the site's HTML to Markdown. Attachment links appear
    /// as `condo-attachment:{attachment_id}`.
    pub body_markdown: String,
    pub attachments: Vec<AttachmentRef>,
}

impl From<ViolationUpdate> for ViolationUpdateOut {
    fn from(u: ViolationUpdate) -> ViolationUpdateOut {
        ViolationUpdateOut {
            update: update_ref(&u),
            posted_at: u.posted_at.map(format_timestamp),
            author: u.author,
            body_markdown: u.body_markdown,
            attachments: u.attachments.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DownloadViolationAttachmentParams {
    pub violation_id: u64,
    /// From get_violation_update's attachments.
    pub attachment_id: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SavedFile {
    /// Absolute path of the saved file, inside the configured download folder.
    pub path: String,
    pub bytes: u64,
    /// The media type storage reported, e.g. image/jpeg.
    pub content_type: Option<String>,
}

impl From<SavedAttachment> for SavedFile {
    fn from(s: SavedAttachment) -> SavedFile {
        SavedFile {
            path: s.path.display().to_string(),
            bytes: s.bytes,
            content_type: s.content_type,
        }
    }
}

#[rmcp::tool_router(router = violations_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_violations",
        description = "List the association's violations, one compact row each (id, name, \
                       type, unit, current stage, due date, last-updated date and who updated \
                       it), sorted by last updated, newest first. Filters: status (open, \
                       closed, draft, past_due, void, resident_updated, all; default open), \
                       search text, stage name, violation_type name, and updated_since \
                       (YYYY-MM-DD). Stage and type names must be ones the site currently \
                       offers in its filters; an unknown name returns the valid list. The \
                       list's last-updated value is a date only, so updated_since compares \
                       dates and keeps same-day updates. limit defaults to 50, max 200. \
                       Without updated_since: total is the site's count for the filters, and \
                       returned < total means limit cut the list. With updated_since: total \
                       is still the site's count for the status/search/stage/type filters \
                       only (before the date filter), so compare against matched instead: \
                       matched is how many scanned rows met the date, and returned < matched \
                       means limit cut the list (raise limit or narrow the filters); \
                       scan_complete false means the 500-row scan cap stopped the scan before \
                       it reached older rows, so more matches may exist. Times are the \
                       association's local time. The \
                       site does not record when a violation's stage changed. Pass an id to \
                       get_violation for its timeline."
    )]
    pub async fn list_violations(
        &self,
        Parameters(p): Parameters<ListViolationsParams>,
    ) -> Result<Json<ViolationList>, ErrorData> {
        let status = parse_status(p.status.as_deref())?;
        let since = p
            .updated_since
            .as_deref()
            .map(|s| {
                NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| {
                    ErrorData::invalid_params(
                        format!("updated_since must be YYYY-MM-DD, got {s:?}"),
                        None,
                    )
                })
            })
            .transpose()?;
        let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

        let (mut stage_ids, mut type_ids) = (Vec::new(), Vec::new());
        if p.stage.is_some() || p.violation_type.is_some() {
            let filters: ViolationFilterOptions = self
                .with_client(|c| c.violation_filters())
                .await
                .map_err(|e| self.tool_error(e))?;
            if let Some(stage) = &p.stage {
                stage_ids = resolve_names(&filters.stages, stage, "stage")?;
            }
            if let Some(t) = &p.violation_type {
                type_ids = resolve_names(&filters.types, t, "violation_type")?;
            }
        }
        let query = ViolationQuery {
            status,
            search: p.search.filter(|s| !s.trim().is_empty()),
            type_ids,
            stage_ids,
            offset: 0,
            limit,
        };

        let Some(since) = since else {
            let page = self
                .with_client(move |c| c.list_violations(&query))
                .await
                .map_err(|e| self.tool_error(e))?;
            let violations: Vec<ViolationRow> = page.rows.into_iter().map(Into::into).collect();
            return Ok(Json(ViolationList {
                total: page.total,
                matched: violations.len(),
                returned: violations.len(),
                scan_complete: true,
                violations,
            }));
        };

        // The site has no date filter: walk newest-first pages until a row is older.
        let (total, kept, scan_complete) = self
            .with_client(move |c| {
                let mut kept = Vec::new();
                let mut total = 0;
                for page_no in 0..MAX_SCAN_PAGES {
                    let q = ViolationQuery {
                        offset: page_no * PAGE_SIZE,
                        limit: PAGE_SIZE,
                        ..query.clone()
                    };
                    let page = c.list_violations(&q)?;
                    total = page.total;
                    let full = page.rows.len() as u32 >= PAGE_SIZE;
                    for row in page.rows {
                        // Rows without a date sort last; treat them as older.
                        if row.last_updated.is_none_or(|d| d < since) {
                            return Ok((total, kept, true));
                        }
                        kept.push(row);
                    }
                    if !full || u64::from(q.offset + PAGE_SIZE) >= total {
                        return Ok((total, kept, true));
                    }
                }
                Ok((total, kept, false))
            })
            .await
            .map_err(|e| self.tool_error(e))?;
        let matched = kept.len();
        let violations: Vec<ViolationRow> = kept
            .into_iter()
            .take(limit as usize)
            .map(Into::into)
            .collect();
        Ok(Json(ViolationList {
            total,
            matched,
            returned: violations.len(),
            scan_complete,
            violations,
        }))
    }

    #[tool(
        name = "get_violation",
        description = "Summarise one violation by id: subject, unit, recipients, type, who \
                       created it, current stage, the recorded Violation Date and due date, \
                       the number of updates, and a timeline of one line per update, newest \
                       first (at most 25; timeline_shown < update_count means the oldest were \
                       left out). Each line gives the update's timestamp, author, opening text \
                       and the `update` value to pass to get_violation_update for its full \
                       text, along with update_count as expected_update_count. Times are the \
                       association's local time, with no zone. The site records only the \
                       current stage, not when stages changed, so this tool does not say when \
                       any notice was sent; read the timeline for that."
    )]
    pub async fn get_violation(
        &self,
        Parameters(p): Parameters<GetViolationParams>,
    ) -> Result<Json<ViolationSummaryOut>, ErrorData> {
        let id = p.id;
        let detail = self
            .with_client(move |c| c.get_violation(id))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(detail.into()))
    }

    #[tool(
        name = "get_violation_update",
        description = "Get one update of a violation in full: author, timestamp (local time) \
                       and the whole body as Markdown, plus the attachments it references, \
                       each with an attachment_id for download_violation_attachment and a \
                       display label when the page gives one. `update` is the value from \
                       get_violation's timeline (today, the update's number with 1 = oldest). \
                       Pass get_violation's update_count as expected_update_count: if the \
                       violation has gained updates since, this fails and asks you to re-run \
                       get_violation rather than return a different update."
    )]
    pub async fn get_violation_update(
        &self,
        Parameters(p): Parameters<GetViolationUpdateParams>,
    ) -> Result<Json<ViolationUpdateOut>, ErrorData> {
        let id = p.id;
        let detail = self
            .with_client(move |c| c.get_violation(id))
            .await
            .map_err(|e| self.tool_error(e))?;
        if let Some(expected) = p.expected_update_count {
            if expected != detail.updates.len() {
                return Err(ErrorData::invalid_params(
                    format!(
                        "Violation {id} now has {} updates, not {expected}. Re-run \
                         get_violation for the current timeline and update values.",
                        detail.updates.len()
                    ),
                    None,
                ));
            }
        }
        let wanted = p.update.trim();
        let update = detail
            .updates
            .into_iter()
            .find(|u| update_ref(u) == wanted)
            .ok_or_else(|| {
                ErrorData::resource_not_found(
                    format!(
                        "Violation {id} has no update {wanted:?}. Use an `update` value from \
                         get_violation's timeline."
                    ),
                    None,
                )
            })?;
        Ok(Json(update.into()))
    }

    #[tool(
        name = "download_violation_attachment",
        description = "Download one attachment of a violation (attachment_id from \
                       get_violation_update) and save it under the server's configured \
                       download folder, named violation-{violation_id}-{attachment_id}-{name} \
                       with an extension from the file's type. Returns the saved path, size in \
                       bytes and content type, never the file contents; open the path to read \
                       it. Re-downloading overwrites the same file. Files over 50 MB are \
                       refused. Images, audio, video and PDFs have been verified to download."
    )]
    pub async fn download_violation_attachment(
        &self,
        Parameters(p): Parameters<DownloadViolationAttachmentParams>,
    ) -> Result<Json<SavedFile>, ErrorData> {
        let dir = self.download_dir.clone();
        let (vid, aid) = (p.violation_id, p.attachment_id);
        let saved = self
            .with_client(move |c| {
                c.download_violation_attachment(vid, aid, &dir, MAX_DOWNLOAD_BYTES)
            })
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(saved.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_collapses_whitespace_and_strips_markdown() {
        let md = "# Notice\n\n**Dear  owner,**\n\n> quoted _text_ here\n\n- item one\n\
                  See [the rules](https://example.com/rules) and \
                  ![photo](condo-attachment:123) then `code` 1\\. done snake_case";
        assert_eq!(
            snippet(md),
            "Notice Dear owner, quoted text here item one See the rules and photo then \
             code 1. done snake_case"
        );
    }

    #[test]
    fn snippet_keeps_short_text_whole() {
        assert_eq!(snippet("  Hello\n\nworld  "), "Hello world");
        assert_eq!(snippet(""), "");
    }

    #[test]
    fn snippet_truncates_on_a_char_boundary() {
        // 156 ASCII bytes then a 3-byte char straddling the 157-byte cut point.
        let md = format!("{}€{}", "a".repeat(156), "b".repeat(50));
        let s = snippet(&md);
        assert!(s.len() <= SNIPPET_MAX, "{} bytes", s.len());
        assert!(s.ends_with('…'));
        assert_eq!(s, format!("{}…", "a".repeat(156)));
        // And when the cut lands exactly after the multibyte char, it is kept.
        let md = format!("{}€{}", "a".repeat(154), "b".repeat(50));
        assert_eq!(snippet(&md), format!("{}€…", "a".repeat(154)));
    }

    #[test]
    fn update_ref_prefers_a_site_id() {
        let mut u = ViolationUpdate {
            ordinal: 2,
            site_id: None,
            author: String::new(),
            posted_at: None,
            body_markdown: String::new(),
            attachments: vec![],
        };
        assert_eq!(update_ref(&u), "2");
        u.site_id = Some("abc".into());
        assert_eq!(update_ref(&u), "abc");
    }

    #[test]
    fn stage_names_resolve_case_insensitively_to_every_id() {
        let opts = vec![
            (1, "Warning".to_string()),
            (2, "Owner Declines Hearing ".to_string()),
            (3, "Warning".to_string()),
        ];
        assert_eq!(
            resolve_names(&opts, " warning", "stage").unwrap(),
            vec![1, 3]
        );
        assert_eq!(
            resolve_names(&opts, "owner declines hearing", "stage").unwrap(),
            vec![2]
        );
        let err = resolve_names(&opts, "nope", "stage").unwrap_err();
        assert!(err.message.contains("Warning; Owner Declines Hearing"));
    }
}
