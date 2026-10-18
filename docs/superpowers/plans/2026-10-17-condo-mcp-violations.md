# condo-mcp violations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add read-only violation tools to `condo-mcp` — `list_violations`, `get_violation`, `get_violation_update`, `download_violation_attachment` — so board-meeting prep can see recent violations, what changed, and the recorded history of any violation heading toward a fine.

**Architecture:** `condo-api` gains a violations domain (JSON list endpoint, HTML detail parser, two-step attachment download) behind a new per-domain trait; `condo-mcp` gains `tools/violations.rs` registering four tools. The per-domain trait split recorded as a follow-up in the foundation plan is done first, because violations is the next domain.

**Tech Stack:** Rust workspace; `reqwest` (blocking), `scraper`, `htmd`, `chrono`, `serde_json`, `rmcp`, `httpmock` (all already workspace dependencies).

**Spec:** `docs/superpowers/specs/2026-10-17-condo-mcp-violations-design.md` (read it first; it holds the discovered endpoints, field names and the post-review decisions this plan relies on).

## Global Constraints

- Read-only: GET only. `scripts/guardrails.sh` must keep passing; the storage fetch in Task 4 is also a GET.
- stdout carries only MCP protocol; logs go to stderr.
- HTML parsers anchor on class names, ids and labelled content, never on position or sibling index. Moved markup is `ClientError::PageStructure`; genuinely absent content is an empty result.
- No milestone inference: nothing is labelled "first notice", "warning" or "fine notice" from update order or wording.
- The attachment `Key` and the pre-signed storage URL never appear in tool output, errors, logs or `Debug` output, and are never cached.
- Download folder default `~/condo-downloads/`, overridable with `--download-dir`; size cap 50 MB; filenames `violation-{id}-{attachment_id}-{sanitised name}`.
- Timestamps are the association's local time with no zone; say so, never convert.
- Fixtures are sanitised: fake names, units, addresses, emails and phones; every file `Key` is `00000000-0000-0000-0000-000000000000`. Probe dumps are never committed (use `--out-dir` under the scratchpad, delete after).
- Gate before every commit that touches Rust: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all --all-features && ./scripts/guardrails.sh`.
- Credentials: never read `~/tokens/`. Live probing goes through `asb condo-control -- bash -c '...'` (see Task 1 Step 1); every use needs Ryan's phone approval and a denial or timeout is final — stop and tell him.
- Commits end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

- A violation with **zero updates** yields an empty timeline, not an error (Task 2).
- An unknown violation id returns the 200 "Invalid address" shell or a 404: the shell must be `PageStructure`, the 404 must be `NotFound`, never an empty violation (Task 2).
- **Secrets in errors:** a failed download must not put the `Key` or the pre-signed URL in the error text or logs (Task 4).
- `updated_since` with date-only ties at a page boundary and with the 500-row scan cap: ties kept, cap reported as incomplete (Task 5).
- Attachment with a hostile name (`../x`, absolute path, empty after sanitising), an oversize body, and a redirect to a non-storage host (Task 4).

---

## File Structure

- Modify `condo-api/src/client/mod.rs` — split `CondoClient` into per-domain traits; add the violations-filter cache field.
- Create `condo-api/src/client/violations.rs` — `ViolationsApi` implementation and the attachment flow.
- Create `condo-api/src/model/violations.rs` — list/detail/filter parsing and types.
- Modify `condo-api/src/{error.rs,model/mod.rs,client/mod.rs,lib.rs}` — exports, new `ClientError::Attachment`.
- Modify `condo-fs/src/vfs.rs`, `condo-fs/src/fs.rs` — bound on `LibraryApi + LoginApi` instead of `CondoClient`; shrink `MockClient`.
- Create `condo-mcp/src/tools/violations.rs`; modify `condo-mcp/src/{lib.rs,server.rs,tools/mod.rs}`, `condo-mcp/tests/server.rs`, `README.md`.
- Create fixtures under `condo-api/tests/fixtures/`: `violations_list_page.html`, `violations_search.json`, `violation_detail.html`, `violation_detail_empty.html`.

---

### Task 1: Split `CondoClient` into per-domain traits

Pure refactor; no behavior change. Done first, as its own commit.

**Files:**
- Modify: `condo-api/src/client/mod.rs`, `condo-api/src/lib.rs`
- Modify: `condo-fs/src/vfs.rs` (bounds and `MockClient`), `condo-fs/src/fs.rs`, `condo-fs/src/bin/condo-fs.rs` if they name `CondoClient`
- Modify: any `use condo_api::CondoClient` in `condo-mcp/src/**` and `condo-api/src/bin/condo-probe.rs`

**Interfaces:**
- Produces (all in `condo_api`, re-exported at the crate root):
  - `trait LoginApi { fn login(&self) -> Result<(), ClientError>; }`
  - `trait LibraryApi { list_folder, file_meta, download_file }` (signatures unchanged)
  - `trait CalendarApi { list_events }`
  - `trait AnnouncementsApi { list_announcements, get_announcement }`
  - `trait CondoClient: LoginApi + LibraryApi + CalendarApi + AnnouncementsApi {}` with a blanket `impl<T: LoginApi + LibraryApi + CalendarApi + AnnouncementsApi> CondoClient for T {}` so existing `use condo_api::CondoClient` call sites that need every method keep compiling.
- `condo-fs`'s `Vfs<C>` is bounded on `C: LibraryApi + LoginApi` (whatever it actually calls); its `MockClient` implements only those two traits and loses its three `unimplemented!` methods.

- [x] **Step 1: Baseline.** Run the Global Constraints gate. Expected: green, 89 tests.
- [x] **Step 2: Split the trait** in `condo-api/src/client/mod.rs` as above, moving each method's doc comment with it. `impl CondoClient for HttpCondoClient` becomes one `impl` per new trait. Task 5's `ViolationsApi` is added later; do not add it here.
- [x] **Step 3: Narrow `condo-fs`.** Change the bounds in `vfs.rs`/`fs.rs` and trim `MockClient`. Expected: `condo-fs` no longer mentions announcements or events anywhere (`grep -rn "announcement\|list_events" condo-fs/src` returns nothing).
- [x] **Step 4: Gate.** Expected: green, still 89 tests.
- [x] **Step 5: Commit** `refactor: split CondoClient into per-domain traits`.

---

### Task 2: Violation list — model and client

**Files:**
- Create: `condo-api/src/model/violations.rs`, `condo-api/src/client/violations.rs`
- Create fixtures: `condo-api/tests/fixtures/violations_list_page.html`, `violations_search.json`
- Modify: `condo-api/src/model/mod.rs`, `condo-api/src/client/mod.rs` (`pub mod violations;`, `ViolationsApi`, cache field), `condo-api/src/lib.rs`

**Interfaces:**
- Produces (in `model::violations`, re-exported at the crate root):
  ```rust
  pub enum ViolationStatus { Open, Closed, Draft, PastDue, Void, ResidentUpdated, All }
  impl ViolationStatus { pub fn site_id(self) -> i32 } // Open 1, Closed 3, Draft 4, PastDue -10, Void 5, ResidentUpdated -20, All -1
  pub struct ViolationQuery { pub status: ViolationStatus, pub search: Option<String>,
      pub type_ids: Vec<u64>, pub stage_ids: Vec<u64>, pub offset: u32, pub limit: u32 }
  pub struct ViolationSummary { pub id: u64, pub name: String, pub violation_type: String,
      pub unit: String, pub stage: String, pub due_date: Option<NaiveDate>,
      pub last_updated: Option<NaiveDate>, pub last_updated_by: Option<String> }
  pub struct ViolationPage { pub total: u64, pub rows: Vec<ViolationSummary> }
  pub struct ViolationFilterOptions { pub workspace_id: u64,
      pub types: Vec<(u64, String)>, pub stages: Vec<(u64, String)> }
  pub fn parse_violation_page(json: &str) -> Result<ViolationPage, ClientError>
  pub fn parse_violation_filters(html: &str) -> Result<ViolationFilterOptions, ParseError>
  ```
- Produces (trait, in `client`): `trait ViolationsApi { fn violation_filters(&self) -> Result<ViolationFilterOptions, ClientError>; fn list_violations(&self, q: &ViolationQuery) -> Result<ViolationPage, ClientError>; }` implemented for `HttpCondoClient`; `violation_filters` fetches `/violationtracking/list-violations/` once and caches the result in a `Mutex<Option<ViolationFilterOptions>>` field on `HttpCondoClient`.

- [x] **Step 1: Capture and sanitise fixtures (live, needs phone approval).** Build the probe (`cargo build -p condo-api --features probe --bin condo-probe`) and run, with `--out-dir` in the scratchpad:
  `asb condo-control -- bash -c './target/debug/condo-probe --credentials <(printf "USERNAME=%s\nPASSWORD=%s\n" "$CONDO_USERNAME" "$CONDO_PASSWORD") --out-dir <scratch> /violationtracking/list-violations/'`, then the same with `--xhr` for
  `"/violationTracking/search-violation-new/?WorkspaceID=<id from the page's #WorkspaceID>&SearchTerm=&SelectedViolationStatusID=-1&SelectedViolationTypeIDs=&SelectedCreatorID=-1&SelectedStageIDs=&userIsJustVoter=False&limit=25&offset=0&sort=LastUpdated&order=desc"`. Strip the probe's header block, sanitise per Global Constraints, keep 3–5 rows including one in each of: Warning, Proposed Fine/Option for Appeal Hearing, and a row whose `LastUpdated` is exactly as the site returns it. **Record, in the commit message, what you learned about the shape of `LastUpdated`** — the discovery notes say it is the date followed by a user name (e.g. `10/17/2026 admin`), not date-only as the spec states; the parser must handle both and keep the trailing text as `last_updated_by`. Also record where the type and stage dropdown options (value and label) live on the list page, and whether stages come from `/violationtracking/get-action-list-multiselect/` instead. If stages are not in the list page HTML, capture that endpoint too and have `parse_violation_filters` take both bodies (adjust the signature and this Interfaces block accordingly). Delete the raw dumps.
- [x] **Step 2: Failing parser tests** in `model/violations.rs` (`#[cfg(test)]`):
  - `parses_rows_and_total` — fixture JSON → `total` equals the fixture's, first row's `id`, `stage`, `unit` match.
  - `last_updated_with_trailing_user_is_split` — `"10/17/2026 admin"` → `last_updated == 2026-10-17`, `last_updated_by == Some("admin")`; `"10/17/2026"` → `last_updated_by == None`.
  - `empty_rows_is_an_empty_page_not_an_error` — `{"total":0,"rows":[]}` → `rows.is_empty()`.
  - `malformed_json_is_a_parse_error` — returns `ClientError::Parse`.
  - `filters_page_yields_workspace_type_and_stage_ids` — from the fixture HTML, `workspace_id` equals the fixture's `#WorkspaceID` value; `types` and `stages` contain known fixture pairs; a page without `#WorkspaceID` → `ParseError::MissingElement`.
  - `status_site_ids_match_the_site` — all seven mappings in the Interfaces block.
- [x] **Step 3: Run; expect FAIL** (`cargo test -p condo-api violations`).
- [x] **Step 4: Implement** the types and parsers. `parse_violation_filters` reads `#WorkspaceID` by id and the type/stage `<option value>` lists by the select ids/classes found in Step 1, never by position. Dates: `%m/%d/%Y`; unparseable → `None`.
- [x] **Step 5: Failing client tests** in `client/violations.rs` using `httpmock` (pattern: `client/announcements.rs`):
  - `list_violations_sends_the_site_query_and_xhr_header` — the mock requires `x-requested-with: XMLHttpRequest` and query params `WorkspaceID`, `SelectedViolationStatusID=-1`, `limit=25`, `offset=0`, `sort=LastUpdated`, `order=desc`, `userIsJustVoter=False`; returns the fixture; assert parsed `total`. The list-page mock is registered too (workspace id).
  - `type_and_stage_ids_are_comma_joined` — `type_ids [3,4]` → `SelectedViolationTypeIDs=3,4`.
  - `filters_are_fetched_once_and_cached` — two `list_violations` calls hit the list page mock exactly once (`.assert_hits(1)`).
  - `list_violations_302_to_login_is_an_auth_error` — same shape as the announcements test.
- [x] **Step 6: Run; expect FAIL.**
- [x] **Step 7: Implement `violation_filters` and `list_violations_once`** with `with_reauth`, redirect → `ClientError::Auth`, and the same `error_for_status` handling as announcements. Add the `ViolationsApi` impl and re-exports, and add `ViolationsApi` to the `CondoClient` supertrait list and its blanket impl.
- [x] **Step 8: Gate; commit** `feat(condo-api): list violations and read their filter options`.

---

### Task 3: Violation detail — parser and client

**Files:**
- Modify: `condo-api/src/model/violations.rs`, `condo-api/src/client/violations.rs`, `condo-api/src/client/mod.rs`, exports
- Create fixtures: `condo-api/tests/fixtures/violation_detail.html` (≥3 updates, one with an inline `access-file.aspx` image), `violation_detail_empty.html` (metadata, zero updates)

**Interfaces:**
- Consumes: Task 2's module and `ViolationsApi`.
- Produces:
  ```rust
  pub struct ViolationAttachment { pub id: u64 /* FileRecordID */, pub name: Option<String>, pub(crate) key: String }
  // Debug is hand-written and prints `key: <redacted>`.
  pub struct ViolationUpdate { pub ordinal: usize /* 1 = oldest */, pub site_id: Option<String>,
      pub author: String, pub posted_at: Option<NaiveDateTime>,
      pub body_markdown: String, pub attachments: Vec<ViolationAttachment> }
  pub struct ViolationDetail { pub id: u64, pub subject: String, pub unit: Option<String>,
      pub recipients: Option<String>, pub violation_type: Option<String>, pub created_by: Option<String>,
      pub stage: Option<String>, pub violation_date: Option<NaiveDateTime>,
      pub due_date: Option<NaiveDateTime>, pub updates: Vec<ViolationUpdate> /* oldest first */ }
  pub fn parse_violation_detail(id: u64, html: &str) -> Result<ViolationDetail, ParseError>
  ```
  and `ViolationsApi::get_violation(&self, id: u64) -> Result<ViolationDetail, ClientError>`.

- [x] **Step 1: Capture `/violationTracking/view-violation-details/{id}?status=-1`** for (a) a fine-track violation with several updates and (b) one with none, via the Task 2 Step 1 method; sanitise. While the raw capture exists, **answer and record in the commit message**: does any element carry a stable per-update identifier (element id, data attribute, edit/delete link id)? If yes, `site_id` is populated from it; if no, `site_id` stays `None` and the tools use `ordinal`. Also: does `?status=` change the response when omitted or wrong (second probe, one fetch with no `status`)? Delete raw dumps.
- [x] **Step 2: Failing parser tests:**
  - `metadata_is_read_by_label_text` — subject, unit, recipients, type, created_by, stage, both dates equal the fixture's values.
  - `updates_are_oldest_first_with_ordinals` — the page lists newest first; `updates[0].ordinal == 1` and is the chronologically oldest; `posted_at` is monotonically non-decreasing.
  - `update_body_is_markdown_and_keeps_inline_image_attachment` — the image update has one attachment with the fixture's `FileRecordID`, and `body_markdown` does not contain the `Key`.
  - `zero_updates_is_an_empty_timeline` — `violation_detail_empty.html` → `updates.is_empty()`, metadata still populated.
  - `the_invalid_address_shell_is_a_structure_error` — `<html><body>Invalid address</body></html>` → `ParseError::MissingElement`.
  - `markup_reordered_still_parses` — wrap the fixture's metadata rows in reversed order (built in the test by string manipulation) → same metadata; proves label-anchoring.
  - `debug_output_never_contains_the_key` — `format!("{:?}", detail)` does not contain the fixture's attachment key.
  - `fixtures_carry_no_real_keys` — every `Key=` value in every violation fixture equals `00000000-0000-0000-0000-000000000000`.
- [x] **Step 3: Run; expect FAIL.**
- [x] **Step 4: Implement `parse_violation_detail`.** Reuse the label-value helper and Markdown conversion from `model/announcements.rs` (extract them to a shared `pub(crate)` module if they are private there — that move is part of this step and must not change announcements behavior). Updates are found by the `#updateRows` container and its per-update tables; the add-update form, stage dropdown and editor scripts are ignored. Attachments: `access-file.aspx?FileRecordID=…&Key=…` links/images inside an update body; `name` from the link text/`alt`/`title` if any.
- [x] **Step 5: Failing client tests:** `get_violation_fetches_by_id_with_status_all` (mock path `/violationTracking/view-violation-details/<id>`, query `status=-1`, asserts parsed `subject`), `get_violation_404_is_not_found`, `get_violation_302_to_login_is_an_auth_error`, `get_violation_200_shell_is_a_structure_error`.
- [x] **Step 6: Run; expect FAIL; implement `get_violation_once`** like `get_announcement_once`; if Step 1 showed `?status=` matters, add the fallback the spec describes. Wire through `with_reauth`.
- [x] **Step 7: Gate; commit** `feat(condo-api): read a violation's metadata and update timeline`.

---

### Task 4: Attachment download

**Files:**
- Modify: `condo-api/src/client/violations.rs`, `condo-api/src/client/mod.rs`, `condo-api/src/error.rs`, exports

**Interfaces:**
- Consumes: `ViolationAttachment { id, name, key }`, `get_violation_once`.
- Produces:
  ```rust
  // error.rs
  ClientError::Attachment(String) // "#[error("attachment error: {0}")]"; message never contains a URL or Key
  // model/violations.rs
  pub struct SavedAttachment { pub path: PathBuf, pub bytes: u64, pub content_type: Option<String> }
  // client
  ViolationsApi::download_violation_attachment(&self, violation_id: u64, attachment_id: u64,
      dest_dir: &Path, max_bytes: u64) -> Result<SavedAttachment, ClientError>
  pub(crate) fn sanitise_filename(name: &str) -> String // pure; never empty, no separators, no leading dots
  pub(crate) fn is_storage_host(host: &str) -> bool      // true only for `amazonaws.com` and subdomains
  ```
  Behavior: re-read the violation (stateless), find the attachment by `id` across all updates (none → `ClientError::NotFound`), request `access-file.aspx?FileRecordID=…&Key=…&mode=preview` on the **authenticated** client (redirects not followed), require a 302 whose `Location` host passes `is_storage_host` (else `Attachment("unexpected download location")`), then GET that URL with a **separate cookie-less** `reqwest::blocking::Client` (built once in `HttpCondoClient::new`, no cookie store, redirects disabled), stream to `dest_dir/violation-{vid}-{aid}-{name}` honoring `max_bytes` (abort and delete the partial file past the cap → `Attachment("file exceeds size limit")`). Name: attachment `name`, else storage `Content-Disposition` filename, else `attachment` + extension from `Content-Type`. `dest_dir` is created if missing; the final canonical path must be inside `dest_dir`.

- [x] **Step 1: Failing pure tests:** `sanitise_filename_strips_separators_and_traversal` (`"../../etc/passwd"` and `"/abs/x.pdf"` and `"a\\b.jpg"` each contain no `/`, `\`, or `..` and are non-empty), `sanitise_filename_never_returns_empty` (`""`, `"..."`, `"///"` → `"attachment"`), `sanitise_filename_keeps_a_normal_name` (`"Notice 1.pdf"` unchanged), `is_storage_host_accepts_s3_only` (`ccc-filedata-prod.s3.ca-central-1.amazonaws.com` true; `evil.com`, `amazonaws.com.evil.com`, `notamazonaws.com` false).
- [x] **Step 2: Failing flow tests** (`httpmock`; the storage "host" is a second `MockServer`, so make the host check injectable: `HttpCondoClient` gets a `pub(crate) storage_host_ok: fn(&str) -> bool` field defaulting to `is_storage_host`, overridden in tests to allow `127.0.0.1`; expose it as `#[doc(hidden)] pub fn with_storage_host_check(self, f: fn(&str) -> bool) -> HttpCondoClient`, which `condo-mcp`'s integration tests also need):
  - `downloads_through_the_redirect_and_saves_the_file` — step 1 mock returns 302 with `Location` to the storage mock; storage returns 3 bytes `image/jpeg`; result `path` exists, content matches, file name starts `violation-<vid>-<aid>-`, `bytes == 3`, `content_type == Some("image/jpeg")`.
  - `storage_request_carries_no_cookies_or_session_headers` — storage mock `.matches` asserts no `cookie` header, while the login mock set one on the main client.
  - `redirect_to_an_unexpected_host_is_refused_and_nothing_is_fetched` — with the default host check, `Location: https://evil.example/x` → `Attachment("unexpected download location")`; the evil host receives zero requests (no mock registered; assert error variant).
  - `oversize_download_is_aborted_and_the_partial_file_removed` — `max_bytes = 2`, 3-byte body → `Attachment(..)`, and `dest_dir` contains no `violation-*` file afterwards.
  - `unknown_attachment_id_is_not_found`.
  - `errors_never_contain_the_key_or_signed_url` — trigger each `Attachment` error above; assert `err.to_string()` contains neither the fixture-style `Key=` value nor `X-Amz`.
  - `filename_falls_back_to_content_disposition_then_content_type` — two cases.
- [x] **Step 3: Run; expect FAIL.**
- [x] **Step 4: Implement** per the behavior above, plus the `ClientError::Attachment` variant. Do not add a redirect-following option to the authenticated client. Log only `FileRecordID` and byte counts, never URLs.
- [x] **Step 5: Gate; commit** `feat(condo-api): download violation attachments through the site's signed redirect`.
- [x] **Step 6: Live check (needs phone approval; do not skip).** Using the Task 2 probe method plus `curl` on the `Location`, confirm the flow on (a) the known inline image and (b) **a non-image attachment (PDF) if any violation has one** — search recent violations' detail pages for `access-file.aspx` links whose update text suggests a notice or letter. Record the result in the PR description. If no non-image attachment can be found, the tool description in Task 5 must say only images have been verified.

---

### Task 5: MCP tools

**Files:**
- Create: `condo-mcp/src/tools/violations.rs`
- Modify: `condo-mcp/src/tools/mod.rs` (`pub mod violations;`), `condo-mcp/src/server.rs` (router merge; `download_dir` field and `with_download_dir`; `tool_error` arm for `ClientError::Attachment` — message is the variant's text, no hints about secrets), `condo-mcp/src/lib.rs` (`--download-dir`, default `~/condo-downloads`), `condo-mcp/src/main.rs`
- Test: `condo-mcp/tests/server.rs`

**Interfaces:**
- Consumes: `ViolationsApi`, all violation types from Tasks 2–4. Tools reach them through `self.with_client(move |c| c.…)`.
- Produces tools (names exactly):
  - `list_violations` params: `status: Option<String>` (`open|closed|draft|past_due|void|resident_updated|all`, default `open`), `search`, `stage` (name), `violation_type` (name), `updated_since: Option<String>` (`YYYY-MM-DD`), `limit: Option<u32>` (default 50, max 200). Output `{ total: u64, returned: usize, scan_complete: bool, violations: [{ id, name, violation_type, unit, stage, due_date, last_updated, last_updated_by }] }`. Unknown `status`/`stage`/`violation_type` → `ErrorData::invalid_params` listing valid names (stage/type names resolved via `violation_filters`, case-insensitive). With `updated_since`: page 50 at a time, sorted by last updated desc, keep rows with `last_updated >= date`, stop at the first older row, scan at most 10 pages (`scan_complete = false` if the cap stopped it before reaching an older row). Description states: timestamps are local time; stage-change dates are not recorded by the site.
  - `get_violation` params `{ id: u64 }`. Output: `{ id, subject, unit, recipients, violation_type, created_by, stage, violation_date, due_date, update_count, timeline_shown, timeline: [{ update: String, posted_at, author, snippet }], note: Option<String> }`. `update` is `site_id` when present else `ordinal` as a decimal string. Timeline newest first, at most 25, `snippet` = first ~160 chars of plain text (whitespace collapsed, Markdown syntax stripped). `note` is set for fine-track stages (`Proposed Fine/Option for Appeal Hearing`, `Owner Declines Hearing`, `Appeal/Hearing Decision`, `Fine Issued or Recinded`) and says that dates of earlier notices and of any fine notice are not recorded as such — read the timeline. No inferred milestones anywhere.
  - `get_violation_update` params `{ id: u64, update: String, expected_update_count: Option<usize> }`. Mismatch with the current count → error telling the caller to re-run `get_violation`. Output `{ update, posted_at, author, body_markdown, attachments: [{ attachment_id: u64, name: Option<String> }] }`. Unknown `update` → `resource_not_found`.
  - `download_violation_attachment` params `{ violation_id: u64, attachment_id: u64 }`. Output `{ path, bytes, content_type }`; never file contents. Description states what has been verified (per Task 4 Step 6) and that the path is under the configured download folder.

- [x] **Step 1: Failing integration tests** in `condo-mcp/tests/server.rs` (helpers `mock_login`/`connect` exist; add `connect_with_download_dir`). Fixtures from `../condo-api/tests/fixtures/`.
  - `violation_tools_are_advertised_with_descriptions` — all four names present, non-empty descriptions.
  - `list_violations_defaults_to_open_and_reports_total` — mock requires `SelectedViolationStatusID=1`; output `total`/`returned` as fixture.
  - `list_violations_unknown_stage_lists_valid_names` — `invalid_params` and the message contains a real fixture stage name.
  - `updated_since_keeps_same_day_ties_and_stops_at_older_rows` — three rows, dates D, D, D-1 → two returned, `scan_complete == true`.
  - `updated_since_reports_incomplete_when_the_scan_cap_is_hit` — mock always returns a full page of rows newer than the date; after 10 pages `scan_complete == false` and exactly 500 rows are scanned (`assert_hits(10)`).
  - `get_violation_returns_a_capped_newest_first_timeline` — fixture with 3 updates: `update_count == 3`, first timeline entry is the newest, `snippet.len() <= 160 + 1`.
  - `get_violation_adds_the_not_recorded_note_on_fine_track_stages_only` — fine-track fixture has `note` containing "not recorded"; a Warning-stage violation has `note == None`.
  - `get_violation_never_labels_milestones` — serialised output does not contain `"first notice"` or `"milestone"` (case-insensitive).
  - `get_violation_update_returns_full_markdown_and_attachment_ids` and `get_violation_update_rejects_a_stale_update_count` and `get_violation_update_unknown_update_is_not_found`.
  - `download_violation_attachment_saves_under_the_configured_dir_and_returns_the_path` — storage as a second mock (host check overridden through `#[doc(hidden)] CondoMcp::with_storage_host_check`, which applies `HttpCondoClient::with_storage_host_check` while the `Arc` is still unique); file exists under the tempdir; output contains no file bytes and no `Key`/`X-Amz`.
- [x] **Step 2: Run; expect FAIL** (`cargo test -p condo-mcp violation`).
- [x] **Step 3: Implement** `tools/violations.rs` following `tools/announcements.rs` (output structs derive `Serialize, schemars::JsonSchema`; `From` conversions; shared `format_timestamp`), register the router in `build_router()`, add `--download-dir`. The plain-text snippet helper is a small pure fn with its own unit tests (`snippet_collapses_whitespace_and_strips_markdown`, `snippet_truncates_on_a_char_boundary` using a multibyte character at the cut).
- [x] **Step 4: Run; expect PASS. Gate.**
- [x] **Step 5: README.** Add the four tools to the `condo-mcp` section: what each returns, that stage-change dates are not recorded by the site, that only currently-listed filters apply, the `--download-dir` flag and default, and what attachment types are verified.
- [x] **Step 6: Final gate; commit** `feat(condo-mcp): add the violation tools`.
- [ ] **Step 7: Tick every checkbox in this plan and in the spec's open items; commit** `docs: record the violations plan as complete`. Then use superpowers:finishing-a-development-branch.
