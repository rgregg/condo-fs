#!/usr/bin/env bash
# Enforces two project-wide guarantees that are otherwise only "claimed" in docs:
#
#   1. Read-only: condo-api and condo-mcp must never issue a POST/PUT/PATCH/DELETE,
#      except the single pre-existing login POST in condo-api/src/client/mod.rs.
#   2. stdout purity: condo-mcp speaks MCP over stdio, so stdout is the wire protocol.
#      Nothing in condo-mcp/src may write to stdout directly (println!/print!/io::stdout).
#
# Run this locally with: ./scripts/guardrails.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail=0

echo "== Guardrail: read-only HTTP =="
# Match .post(/.put(/.patch(/.delete( calls, then exclude the one known-good login POST.
mutating_calls=$(grep -rnE '\.(post|put|patch|delete)\(' condo-api/src condo-mcp/src || true)
violations=$(echo "$mutating_calls" | grep -v '^condo-api/src/client/mod\.rs:.*\.post(self\.url("/login/login-post"))' | grep -v '^$' || true)
if [ -n "$violations" ]; then
    echo "FAIL: found a non-GET HTTP call outside the known login POST." >&2
    echo "" >&2
    echo "condo-api and condo-mcp are read-only by design: the only mutating request this" >&2
    echo "project is allowed to make is the single login POST to /login/login-post in" >&2
    echo "condo-api/src/client/mod.rs. Every other call must be a GET." >&2
    echo "" >&2
    echo "If you are adding a genuinely new, reviewed write path, update the allowlist in" >&2
    echo "scripts/guardrails.sh alongside your change and explain why in the PR — do not" >&2
    echo "just delete this check." >&2
    echo "" >&2
    echo "Offending line(s):" >&2
    echo "$violations" >&2
    fail=1
else
    echo "OK: no mutating HTTP calls outside the known login POST."
fi

echo
echo "== Guardrail: condo-mcp stdout purity =="
stdout_hits=$(grep -rnE '(println!|print!|io::stdout)' condo-mcp/src || true)
if [ -n "$stdout_hits" ]; then
    echo "FAIL: found a direct stdout write in condo-mcp/src." >&2
    echo "" >&2
    echo "condo-mcp is an MCP stdio server: stdout is the JSON-RPC wire protocol, and any" >&2
    echo "stray byte written to it (via println!/print!/io::stdout) corrupts every message" >&2
    echo "after it for the connected client. All logging must go to stderr (the 'log'" >&2
    echo "crate's macros, which condo-mcp already routes to stderr, are fine)." >&2
    echo "" >&2
    echo "Offending line(s):" >&2
    echo "$stdout_hits" >&2
    fail=1
else
    echo "OK: no direct stdout writes in condo-mcp/src."
fi

if [ "$fail" -ne 0 ]; then
    echo
    echo "Guardrails failed. See messages above." >&2
    exit 1
fi

echo
echo "All guardrails passed."
