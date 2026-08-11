# condo-mcp design

Date: 2026-08-10
Status: approved

## Purpose

Expose Condo Control association data to MCP clients (Claude Code, Claude Desktop) as
read-only query tools, so existing workflows — board meeting prep, ad-hoc questions about
the community, drafting correspondence, and monitoring what changed — can pull real data
instead of being hand-fed screenshots and copy-paste.

Condo Control publishes no API. `condo-fs` already reverse-engineered the parts needed to
read the File Library: a multipart login that yields a session cookie, and the convention
that a 302 to `/login` means the session expired. `condo-mcp` reuses that work rather than
re-deriving it.

## Scope

Condo Control has seven areas worth exposing: files, calendars, discussion forums, phone
book, service requests, violations, and announcements. These are **not one project** — each
has distinct endpoints, JSON shapes, and pagination that must be mapped separately.

**Phase 1 (this spec):** workspace refactor, MCP server skeleton, announcements, calendar.

The skeleton carries one file-library tool, `list_folder`, ahead of the files phase. It costs
roughly thirty lines against a client that is already written and tested, and it is the only
way to exercise the shared-session, `spawn_blocking`, and lazy-login design end-to-end
against a real endpoint before parsers are written for endpoints nobody has seen yet. A
skeleton with no tools cannot be tested as a server.

**Later phases (each gets its own spec):** files (cheapest — the client exists), phone book,
discussion forums, service requests, violations.

Phase 1 deliberately includes the two areas with the highest signal for meeting prep and
the simplest likely response shapes, so the server skeleton is proven against real data
before the harder domains land.

### Non-goals

- **No writes of any kind.** Every tool is a query. No POST, no form submission, no
  anti-forgery token handling. An agent must not be able to post to the community forum,
  file a service request, or RSVP on the user's behalf.
- No caching layer beyond what a single process needs. `condo-fs` has a content cache
  because FUSE demands one; MCP calls are infrequent and interactive.
- No multi-association support. One credentials file, one association.

## Architecture

A three-crate Cargo workspace, replacing today's single crate:

```
condo-fs/                      workspace root
├── Cargo.toml                 [workspace] members = ["condo-api", "condo-fs", "condo-mcp"]
├── condo-api/                 shared, transport-agnostic
│   └── src/
│       ├── lib.rs
│       ├── credentials.rs     moved verbatim from condo-fs
│       ├── paths.rs           expand_tilde, moved out of condo-fs::config
│       ├── error.rs           ClientError
│       ├── client.rs          CondoClient trait, HttpCondoClient, login, with_reauth
│       ├── client/library.rs      list_folder, file_meta, download_file
│       ├── client/announcements.rs
│       ├── client/calendar.rs
│       └── model/{library,announcements,calendar}.rs
├── condo-fs/                  behavior unchanged
│   └── src/{fs,vfs,cache,names,config}.rs, bin/condo-fs.rs
└── condo-mcp/
    └── src/{main.rs, server.rs, tools/{announcements,calendar}.rs}
```

`condo-api` owns the hard-won part: the login flow, the cookie store, and `with_reauth`
(retry exactly once on `ClientError::Auth`, then give up — never loop). Both binaries share
one implementation of it.

`condo-api` stays on **blocking** `reqwest`. FUSE is a blocking API and `condo-fs` should
not be perturbed by this work. `condo-mcp` is a `tokio` binary that wraps each call in
`tokio::task::spawn_blocking`, holding one `Arc<HttpCondoClient>` for the process lifetime
so all tools share a single authenticated session.

The `CondoClient` trait grows one method per domain rather than sprouting a second client
type. That preserves the existing test pattern — `httpmock` server plus a JSON fixture —
for every new domain.

### Crate boundaries

| Crate | Does | Depends on |
|---|---|---|
| `condo-api` | Authenticate, fetch, parse into typed models | `reqwest`, `serde`, `thiserror`, `log`, `chrono`/`chrono-tz` (timestamps), `html2md` (HTML bodies) |
| `condo-fs` | Map models onto a FUSE tree, cache content | `condo-api`, `fuser`, `libc`, `clap` |
| `condo-mcp` | Expose models as MCP tools over stdio | `condo-api`, `rmcp`, `tokio`, `clap` |

Neither binary crate depends on the other. `condo-api` knows nothing about FUSE or MCP.

## Endpoint discovery

Endpoints for announcements and calendar are unknown and must be found by probing the live
site with the user's own credentials. The user is a legitimate account holder; the probing
is authorized use of their own account, but it still runs against production and must be
conservative.

A dev-only binary, `condo-api/src/bin/condo-probe.rs`, does this:

- Logs in using the existing flow.
- Issues **GET requests only**. The probe has no code path that can emit any other method —
  this is a structural guarantee, not a convention.
- Runs one request at a time with a fixed delay between requests. No concurrency, no retry
  storms.
- Dumps status, headers, and body for each path to a file under `probe-out/`.
- Takes paths from the command line or a list file, so exploration is explicit and
  reviewable, never a generated crawl.

Candidate paths are guessed from the patterns already confirmed in the File Library
(`/library/get-file-list` behind an `X-Requested-With: XMLHttpRequest` header returning bare
JSON arrays). Likely shapes: `/announcements/get-announcement-list`,
`/calendar/get-event-list`, and the HTML pages `/announcements`, `/calendar` that reference
them.

Every useful response is trimmed and committed to `condo-api/tests/fixtures/` — with member
names, unit numbers, phone numbers, and email addresses replaced by plausible fakes. Real
resident data never enters git. Parsers are then written and tested offline against those
fixtures, so the test suite never touches the network.

## Tool surface

Tools are general-purpose per domain (list / get), not task-shaped. The four use cases —
prep, Q&A, drafting, monitoring — are all served by the same primitives; the task shaping
already lives in the user's skills layer, and duplicating it here would fossilize today's
workflows into the protocol.

### Announcements

**`list_announcements`** — `{ limit?: int = 25, since?: date, until?: date }`
Returns `[{ id, title, posted_at, author, summary }]`, newest first. `summary` is the first
~200 characters of the body as plain text.

**`get_announcement`** — `{ id: int }`
Returns `{ id, title, posted_at, author, body, attachments: [{ id, name }] }`. `body` is the
announcement HTML converted to Markdown. Attachments are **listed only** — fetching their
content is a files-phase concern, and the tool description says so explicitly so an agent
does not report an attachment as unreadable.

### Calendar

**`list_events`** — `{ start: date, end: date, calendar?: string }`
Returns `[{ id, title, start, end, all_day, location, calendar }]` sorted by start time.
`start`/`end` default to today and today + 60 days.

**`get_event`** — `{ id: int }`
Returns `{ id, title, start, end, all_day, location, calendar, description }`.

### Conventions across all tools

- All timestamps are ISO-8601 with an explicit offset. Condo Control returns naive strings
  like `2025-01-18 02:41:25` with no zone; these are interpreted as the association's local
  time, set by a `--timezone` flag that defaults to the host's local zone. Parsing and
  normalizing is the client's job, not the caller's.
- All HTML bodies are converted to Markdown before returning. Agents should never receive
  raw markup.
- Results are returned as structured JSON content.
- Every list tool documents its ordering in the tool description, because agents summarizing
  "what's new" depend on it.

## Configuration

`condo-mcp` takes the same credentials file as `condo-fs` (`~/tokens/condo-control.txt` by
default, `KEY=VALUE` lines) and a `--base-url` defaulting to `https://app.condocontrol.com`.
It runs as a stdio server, registered in the MCP client config.

Login is lazy: the first tool call authenticates. A server that fails to start because the
credentials file is missing would be indistinguishable, in most MCP clients, from a broken
install.

## Error handling

`ClientError` maps to MCP tool errors with messages written for an agent to act on:

| Condition | Surfaces as |
|---|---|
| `Auth` after one retry | "Authentication failed — check the credentials file at `<path>`." |
| `NotFound` | "No announcement with id N." — a normal result, not a crash. |
| `Http` / timeout | "Condo Control is unreachable: `<detail>`." Retryable, and says so. |
| `Parse` | "Unexpected response shape from Condo Control." Logs the raw body at debug level. |

The password is never logged, never included in an error message, and never serialized —
`Credentials` keeps its hand-written `Debug` impl that redacts it. A parse failure logging a
raw body must log the *response* body, never a request body.

A tool failure returns an error to that call only; it never terminates the server.

## Testing

- **Parser unit tests** against committed, anonymized fixtures — the same pattern as
  `parse_file_list` today.
- **Client tests** with `httpmock`: success, 302-to-login producing `Auth`, and the
  "re-auth exactly once, then give up" invariant, per new domain.
- **Tool tests** asserting each tool's output JSON matches its declared schema, including
  the empty-result case (no announcements in range must return `[]`, not an error).
- **Live smoke test**, opt-in behind an env var, matching the existing `tests/live_smoke.rs`
  pattern. Not run in CI.
- The refactor is verified by `condo-fs`'s existing suite passing unchanged after the move.
  Fixture paths shift to `condo-api/tests/fixtures/`; no test logic changes.

## Risks

- **The endpoints may not be JSON.** Some areas may render server-side HTML with no XHR
  endpoint behind them, forcing HTML scraping. Probing resolves this before any parser is
  written; if announcements turn out to be HTML-only, the model layer absorbs it and the
  tool surface is unaffected.
- **Undocumented endpoints change without notice.** Mitigated by parse errors being
  explicit and loud rather than silently yielding empty results — an agent must never be
  told "no announcements" when the truth is "the parser broke."
- **Permission-gating.** Some areas may return different shapes for board members versus
  residents. Phase 1's two areas are the least likely to be gated; the harder cases land in
  later specs.
