# condo-mcp owner directory design

## Purpose

Add the Condo Control owner directory (the SPA's `/spa/my/phone-book`) to `condo-mcp` as one
read-only tool, so an agent can answer "who is the owner of unit X" and "what is this person's
contact information" while Ryan prepares board work (violations name units, not people).

## Goals

- `search_directory`: look an owner up by name or unit and return compact entries: unit, name,
  phone numbers (with type), email, on-site address.
- State the site's total for the search, so a truncated list is never mistaken for a complete one.

## Non-goals and anti-goals

- No writes, no export of the whole directory as a dump tool, no caching of directory data.
- Do not guess at fields the site does not return (off-site address was null in every probed
  row, so it is not exposed).
- Tool output is personal contact data. It goes only to the invoking agent; it is not logged, and
  fixtures are synthetic.

## What Condo Control exposes (read-only probe, November 2026)

The SPA page is a shell. Data comes from `GET /web/PhoneBook/Directory` (`/web` is the SPA's core
API base), with the normal session cookie and `Accept: application/json`.

Query: `searchTerm` (matches unit and name; not email, phone or street; `search`, `q`, `query`
and `name` are silently ignored), `page` (1-based), `pageSize` (25 default; 100 works; 500 is HTTP
400). Response: `{ data: [...], pagination: { page, pageSize, totalItems, totalPages,
hasNextPage, hasPreviousPage } }`. A row: `unitName`, `firstName`, `lastName`,
`phoneNumbers: [{ number, type }]` (Cell, Home, Other), `emailAddress` (sometimes null),
`onSiteAddress: { lines: [..], countryLocalizationKey }` (often null), `offSiteAddress` (null).
314 entries at probe time. One owner can appear on several rows.

## Tool

`search_directory { search?, unit?, limit?, offset? }`

- `search` is sent as `searchTerm`. `unit` filters to rows whose unit name equals it
  (case-insensitive); if only `unit` is given it is also sent as the search term.
- `limit` defaults to 50, max 200. Pages of 100 are fetched until `limit` rows are collected, the
  site has no more, or 10 pages have been scanned.
- `offset` (added after the first live run showed `limit` alone cannot reach past 200 of 314
  entries) skips that many *matching* entries. With no `unit` filter it maps straight to a site
  page and a skip within it; with one, the scan starts from the first page. The output echoes
  `offset` and gives `next_offset` only when `limit` cut the list.
- Output: `total` (site count for the search term), `matched`, `returned`, `scan_complete`, and
  `entries`. With no `unit` filter `matched == total` whenever the scan is complete.
- No parameters at all lists the first `limit` entries; the description says so.

## Approaches considered

1. One search tool over the JSON endpoint (chosen): the endpoint is already filtered and paged by
   the site, so the tool is a thin layer.
2. Scrape the legacy HTML directory: rejected, no reason to when JSON exists.
3. Separate `get_owner` and `list_directory` tools: rejected for now; unit lookup is a filter on
   the one tool, and two tools duplicate a code path.

## Builder's calls (for Ryan to veto)

- Email and phone numbers are included by default; reaching an owner is the point.
- `unit` is an exact, case-insensitive match applied by the tool.
- Auth errors: a redirect or 401 is `ClientError::Auth` and triggers the normal re-login.

## Testing

Parser tests against a synthetic fixture (phones missing, email missing, address missing,
multiple phones); mock-server tests for the request shape (`searchTerm`, `page`, `pageSize`,
`Accept`) and auth redirect; MCP tests for paging, the unit filter, truncation and the scan cap.
Gate: fmt, clippy `-D warnings`, tests, guardrails. Tests never touch the network.
