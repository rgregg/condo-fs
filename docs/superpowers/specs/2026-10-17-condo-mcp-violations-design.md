# condo-mcp violations design

## Purpose

Ryan prepares for HPMA board meetings by reviewing recent violations, checking what has changed on
existing ones, and making sure he is well briefed on the history of any violation the board is
moving toward a fine. The deciding question for a violation on the fine track is its **timeline**:
when the owner was first notified, what follow-up and conversation happened, and when a fine
notice went out.

This adds violations to `condo-mcp` as read-only tools so an agent can assemble that briefing.
It is the second HTML-heavy domain after announcements, and follows the rule that design
established: anchor parsers on class names and content, never on position, and make moved markup a
`ParseError` while genuinely absent content stays an empty result.

## Goals

- List violations with a compact row each, filterable by status, so "what's recent" and "what is
  open" are one call.
- Answer "what changed since I last looked" using an updated-since date.
- Give a **short summary** of one violation: metadata, current stage, and a compact timeline, one
  line per update, so a briefing can cover many violations cheaply.
- Give the **full text of one update** on demand.
- Download an attachment to a local folder and return its path.

## Non-goals and anti-goals

- **No writes of any kind.** No posting updates, changing stages, setting due dates, uploading
  files, or deleting drafts. The site exposes all of these; none are in scope. The server stays
  GET-only, like the rest of `condo-mcp`.
- **Do not infer or label milestones** (first notice, warning, fine notice) from update order or
  wording. See "Milestones" below. A wrong "first notice" date in a board briefing is worse than a
  missing one.
- No tool returns every update's full text for a violation in a single call; that is the
  token-heavy shape Ryan explicitly chose against.
- No caching of attachment links (see Attachments).

## What Condo Control exposes (discovered by read-only probing, October 2026)

All paths need the logged-in session. 918 violations existed at the time of probing.

**List.** `GET /violationTracking/search-violation-new/` returns JSON `{ "total": N, "rows": [...] }`
and needs the `X-Requested-With: XMLHttpRequest` header. It is the data source for the
bootstrap-table on `/violationtracking/list-violations/`. Query parameters: `WorkspaceID`
(the association's workspace id, read from a hidden input on the list page; do not hard-code it),
`SearchTerm`, `SelectedViolationStatusID`, `SelectedViolationTypeIDs` (comma-separated),
`SelectedCreatorID` (`-1` for any), `SelectedStageIDs` (comma-separated), `userIsJustVoter`
(`False`), and paging/sort `limit`, `offset`, `sort`, `order`. Status ids: Open 1, Closed 3,
Draft 4, Past Due -10, Void 5, Last Updated By Resident -20, All -1. Each row has `ViolationName`,
`ViolationType`, `Unit`, `Stage`, `DueDate`, `LastUpdated`, `ObjectNumber`, `ViolationId`.
`LastUpdated` in the list is date-only; row-level times come only from the detail page.

**Detail.** `GET /violationTracking/view-violation-details/{ViolationId}?status={n}` is
server-rendered HTML. Labelled metadata: Unit Involved, Recipients, Violation Type, Created By,
Violation Stage, Violation Date, Subject, Due Date. Updates are repeated `<table>` blocks inside
`#updateRows`, newest first, each with an author, a `MM/DD/YYYY hh:mm AM/PM` timestamp and an HTML
body in a `divUpdateText` cell. The page also contains an add-update form, stage dropdown and
editor scripts that must be ignored, and must never be submitted.

**Stages** (the dropdown): Reported, Warning, 15 Day Warning, 30 Day Warning, Recurring Violation
within a Six Month Period, Garbage Violation, Non-Violation Incident, Proposed Fine/Option for
Appeal Hearing, Owner Declines Hearing, Appeal/Hearing Decision, Fine Issued or Recinded.

**Attachments.** The only attachment seen was an image inline in an update body, as
`access-file.aspx?FileRecordID=…&Key=…&mode=preview`. With the session that URL answers `302` to a
pre-signed S3 URL (one-hour expiry) on a different host; a plain unauthenticated GET of the S3 URL
returned the file (`image/jpeg`, 56 KB). Not yet seen: attachments listed separately from the body,
and any non-image type.

**Timezone.** Detail timestamps carry no zone. The calendar domain found the association's feeds
use local time; assume the same here and state it rather than converting.

## Tools

### `list_violations`

Compact rows: id, name, type, unit, stage, due date, last updated. Parameters (all optional):
`status` (open | closed | draft | past_due | void | resident_updated | all), `search`,
`stage`, `type`, `updated_since` (date), `limit`.

- **Default status is `open`**, sorted by last updated, newest first. *(Builder's call.)*
- `updated_since` is applied by the tool, not the site (the site has no such filter): page through
  results newest-first and stop once rows are older than the date. Because the list's last-updated
  value is date-only, compare by date; say so in the tool description.
- Output states the site's `total` for the chosen filter and how many rows are returned, so a
  truncated list is never mistaken for a complete one.

### `get_violation`

The short summary: metadata, current stage, due date, number of updates, and a compact timeline of
one line per update: timestamp, author, and the update's opening text (about 160 characters, plain
text, whitespace collapsed). Each timeline line carries the identifier `get_violation_update`
accepts (see "Update addressing" below). Timeline is shown newest first and capped (default 25
lines); when capped, the output says how many were omitted.

### `get_violation_update`

Takes a violation id and an update index. Returns that update's author, timestamp and full body
converted to Markdown (reusing the announcements HTML-to-Markdown path), plus the attachments found
in it, each with a name or best available label, a type if known, and an `attachment_id` for the
download tool.

### `download_violation_attachment`

Takes a violation id and an `attachment_id`; saves the file to the download folder and returns the
saved path, size and content type. It never returns file contents inline. See Attachments.

### Milestones: none are inferred

Condo Control records no stage-change history that we found, so "first notice", "warning" and
"fine notice" dates cannot be read directly, and **the tools do not guess them**. Ryan's decision:
the earliest update is not reliably the first notice to the owner. `get_violation` reports only
recorded facts: the Violation Date field, the current stage, the due date, and the timeline of
updates with their own timestamps. For a violation on a fine-track stage the summary states
plainly that the dates of earlier notices and of any fine notice are not recorded as such, and
points at the timeline. The agent reading the timeline, and Ryan, draw the conclusions.

## Attachments

- **Two hosts.** Step one is the authenticated `access-file.aspx` request with redirects *not*
  followed automatically. Step two is a plain GET of the `Location` URL. The step-two request must
  **not** carry the Condo Control cookies or any session header, and must be a separate code path
  from the authenticated client, not a redirect-following option on it.
- **Validate before following.** Only follow a redirect whose host is the expected storage host
  family (an `amazonaws.com` S3 host); otherwise return an error rather than fetching an arbitrary
  URL a response told us to fetch.
- **Never cache or return the pre-signed URL.** It is a bearer credential for the file for an hour.
  Fetch it fresh per download, and do not log it or include it in errors or tool output. The same
  applies to the `Key` query value.
- **Download folder.** Default `~/condo-downloads/` *(builder's call)*, overridable with
  `--download-dir` on `condo-mcp`. Created on first use. Files are named
  `violation-{id}-{attachment_id}-{sanitised-original-name}` so names are deterministic, cannot
  collide across violations, and cannot escape the folder (reject path separators and `..`).
  Re-downloading the same attachment overwrites. Size cap with a clear error (default 50 MB).
- **Types.** Not just images: the download path must stream bytes and not assume JPEG. PDFs and
  other types are expected for violations (notices, photos from owners) but are **unverified**; the
  implementation plan must include a live check against a real non-image attachment before the
  tool is described as supporting them.
- **Listing.** If a violation or update lists attachments outside the body, parse them the same
  way. If only inline image links are ever found, say so in the tool description rather than
  implying a general attachment list exists.

## Architecture

Follows the existing split. `condo-api` gets `client/violations.rs` and `model/violations.rs`
(list JSON, detail HTML parser, attachment resolver) and new `CondoClient` methods;
`condo-mcp` gets `tools/violations.rs` registering the four tools. HTML parsing reuses `scraper`
and `htmd` and the `ParseError` type. Fixtures are sanitised captures under
`condo-api/tests/fixtures/` (see Privacy).

`CondoClient` already carries 8 methods and the previous PR recorded that it should be split into
per-domain traits before the next domain lands. Violations is that next domain, so the plan should
do the split first, as its own commit, before adding the violation methods. *(Builder's call; it
follows the recorded follow-up.)*

## Constraints

- Read-only, GET-only, enforced by the existing guardrails script. The attachment step-two fetch is
  also a GET.
- stdout carries only MCP protocol; logging goes to stderr.
- Errors: not-found for an unknown violation id, an auth error on a redirect to login, and a
  `ParseError` when expected markup moves. Missing optional content (no updates, no attachments) is
  an empty result, not an error.

## Privacy

Violation data names owners, units and addresses. Fixtures must be sanitised: replace names,
unit numbers, addresses, emails, phone numbers and any file keys with obviously fake values before
committing. Probe dumps are never committed. Tests never touch the network. Tool output is
returned only to the invoking agent; it is not logged by the server.

## Testing

Parser tests against sanitised fixtures, each with a case built to fail against a positional
selector (the announcements rule). Mock-server tests for the list (including paging and
`updated_since`), the detail page, and the two-step attachment flow, including: a redirect to an
unexpected host is refused, cookies are absent on step two, and a path-traversal filename is
neutralised. The existing gate applies: fmt, clippy `-D warnings`, tests, guardrails.

## Decisions made after the builder check (builder's calls, for Ryan to veto)

- **Update addressing.** If the detail page carries a stable per-update identifier (an element id,
  data attribute, or edit-link id), use it instead of an index; the plan must check the fixture for
  one. If none exists, number updates **oldest-first** (update 1 is the first), so a newly posted
  update never renumbers existing ones, and have `get_violation` return the update count.
  `get_violation_update` accepts that count as an optional guard and errors if it no longer matches.
- **Attachment ids.** `attachment_id` is the `FileRecordID` only. The `Key` never leaves the server
  and is never stored: `download_violation_attachment` re-reads the violation's detail page, finds
  the attachment with that `FileRecordID`, and takes the `Key` from the page at that moment. This
  also proves the attachment belongs to the stated violation. "Never cache" covers the `Key` and the
  pre-signed URL both.
- **Download filenames.** Use the original name if the page gives one; otherwise take it from the
  storage response's `Content-Disposition`; otherwise `attachment` plus an extension from the
  content type. `~` in `--download-dir` is expanded with the existing `expand_tilde`. A download
  directory that is a symlink is allowed, but the final file path must stay inside it.
- **`updated_since` and `limit`.** Page size 50, `limit` defaults to 50, and at most 10 pages
  (500 rows) are scanned for `updated_since`. If the scan stops at the cap, the output says so and
  says it is incomplete. Rows are sorted by the site's `LastUpdated` descending; date-only ties are
  kept (compare with `>=`).
- **`stage` and `type` filters** take names. The plan resolves them to ids from the list page's
  type and stage dropdowns (and `/violationtracking/get-action-list-multiselect/` if the stages are
  loaded from there), and returns a clear error listing valid names for an unknown one. The plan
  must verify where those ids actually come from before relying on this.
- **Detail `?status=`.** Start by sending `-1` (All). The plan must verify with a read-only probe
  whether a wrong or missing value changes the response; if it does, `get_violation` falls back to
  the list row's status.
- **Violation Date** is a recorded field and is shown as such. It is not described as the date of
  first notice.
- **Guardrails.** `scripts/guardrails.sh` only forbids non-GET calls and stdout writes, so a second,
  cookie-less client used for the storage GET needs no script change.

## Left to the builder

Exact tool parameter names not stated above, the shape of the timeline line, how to read
`WorkspaceID` (fetch the list page once per session and cache it), and the HTML-to-text truncation
algorithm.
