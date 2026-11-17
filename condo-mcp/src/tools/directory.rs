use crate::server::CondoMcp;
use condo_api::{DirectoryApi, DirectoryEntry, DirectoryQuery};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

/// Rows per request. The site answers HTTP 400 for a much larger page size.
const PAGE_SIZE: u32 = 100;
/// At most this many pages (1000 rows) are scanned for one search.
const MAX_PAGES: u32 = 10;
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchDirectoryParams {
    /// Text to look for. The site matches unit numbers and owner names only; email,
    /// phone and street are not searched.
    pub search: Option<String>,
    /// Only entries on this unit (exact match, ignoring case), e.g. "101".
    pub unit: Option<String>,
    /// Most entries to return. Default 50, max 200.
    pub limit: Option<u32>,
    /// Matching entries to skip first, for paging past `limit`. Default 0. Pass the
    /// previous result's `next_offset`.
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DirectoryPhoneOut {
    pub number: String,
    /// The site's label: Cell, Home or Other.
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DirectoryRow {
    pub unit: String,
    pub first_name: String,
    pub last_name: String,
    pub phones: Vec<DirectoryPhoneOut>,
    /// Absent when the owner has none listed.
    pub email: Option<String>,
    /// The on-site mailing address, one string per line; empty when none is listed.
    pub address: Vec<String>,
}

impl From<DirectoryEntry> for DirectoryRow {
    fn from(e: DirectoryEntry) -> DirectoryRow {
        DirectoryRow {
            unit: e.unit,
            first_name: e.first_name,
            last_name: e.last_name,
            phones: e
                .phones
                .into_iter()
                .map(|p| DirectoryPhoneOut {
                    number: p.number,
                    kind: p.kind,
                })
                .collect(),
            email: e.email,
            address: e.on_site_address,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DirectoryResult {
    /// The site's count for the search text alone (before the unit filter).
    pub total: u32,
    /// Entries in `entries`.
    pub returned: usize,
    /// False when `limit` or the 1000-row scan cap cut the results short: more matching
    /// entries may exist. True means every match was returned.
    pub complete: bool,
    /// Matching entries skipped before the first returned one.
    pub offset: u32,
    /// Pass as `offset` to get the next entries. Present only when `limit` cut the list;
    /// absent when everything was returned or the scan cap stopped the scan.
    pub next_offset: Option<u32>,
    pub entries: Vec<DirectoryRow>,
}

#[rmcp::tool_router(router = directory_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "search_directory",
        description = "Search the association's owner directory (Condo Control's phone book): \
                       unit, owner name, phone numbers with their type (Cell, Home, Other), \
                       email and on-site address. Use it to find who owns a unit or how to \
                       reach an owner. `search` matches unit numbers and owner names only, not \
                       email, phone or street. `unit` keeps only entries on that exact unit \
                       (ignoring case). With neither, it lists the first `limit` entries. \
                       One owner can appear on several entries, and phone, email and address \
                       are often missing. limit defaults to 50, max 200. total is the site's \
                       count for `search` alone; complete false means limit or the 1000-row \
                       scan cap cut the results short. offset skips that many matching \
                       entries first; when limit cut the list, next_offset is the offset \
                       for the next call. This is personal contact information \
                       about residents: use it only for the question at hand and do not \
                       repeat it beyond what is needed."
    )]
    pub async fn search_directory(
        &self,
        Parameters(p): Parameters<SearchDirectoryParams>,
    ) -> Result<Json<DirectoryResult>, ErrorData> {
        let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
        if limit == 0 || limit > MAX_LIMIT {
            return Err(ErrorData::invalid_params(
                format!("limit must be between 1 and {MAX_LIMIT}."),
                None,
            ));
        }
        let unit = p
            .unit
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        let search = p
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // The site's search covers unit names, so a unit lookup alone narrows the request
        // up front; the exact-match filter below removes substring matches like 10 -> 101.
        let term = search.or_else(|| unit.clone());
        let offset = p.offset.unwrap_or(0);
        // Without a unit filter every site row matches, so an offset maps straight onto a
        // page and a skip within it, and the request can start there. With a unit filter
        // the matches are only known after filtering, so scan from the start.
        let page_size = if unit.is_some() || offset > 0 {
            PAGE_SIZE
        } else {
            limit.min(PAGE_SIZE)
        };
        let (mut page_no, mut to_skip) = if unit.is_some() {
            (1, offset)
        } else {
            (offset / page_size + 1, offset % page_size)
        };

        let mut entries: Vec<DirectoryRow> = Vec::new();
        let mut total = 0u32;
        let mut complete = true;
        let mut cut_by_limit = false;
        let mut scanned = 0u32;
        loop {
            let q = DirectoryQuery {
                search: term.clone(),
                page: page_no,
                page_size,
            };
            let page = self
                .with_client(move |c| c.list_directory(&q))
                .await
                .map_err(|e| self.tool_error(e))?;
            scanned += 1;
            if scanned == 1 {
                total = page.total_items;
            }
            let rows = page.entries.into_iter().filter(|e| match unit {
                Some(ref u) => e.unit.eq_ignore_ascii_case(u),
                None => true,
            });
            for e in rows {
                if to_skip > 0 {
                    to_skip -= 1;
                    continue;
                }
                if entries.len() == limit as usize {
                    // A further match exists beyond the limit.
                    complete = false;
                    cut_by_limit = true;
                    break;
                }
                entries.push(e.into());
            }
            if cut_by_limit {
                break;
            }
            if !page.has_next_page {
                break;
            }
            if entries.len() == limit as usize && unit.is_none() {
                // Exactly `limit` so far and the site has more rows. Without a unit
                // filter every row matches, so more rows means truncated.
                complete = false;
                cut_by_limit = true;
                break;
            }
            if scanned >= MAX_PAGES {
                complete = false;
                break;
            }
            page_no += 1;
        }
        let next_offset = cut_by_limit.then(|| offset + entries.len() as u32);
        Ok(Json(DirectoryResult {
            total,
            returned: entries.len(),
            complete,
            offset,
            next_offset,
            entries,
        }))
    }
}
