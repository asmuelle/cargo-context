#!/usr/bin/env bash
# PreToolUse guard for the Write tool.
#
# Blocks writes whose content exceeds 600 lines — the same limit enforced by
# `scripts/qa/check_file_lengths.py`. Aligning the hard cap with the baseline
# limit prevents the previous footgun where a 700-line write would pass the
# guard and then immediately fail the post-edit length check.
#
# Pre-existing oversized files live at their `file_length_baseline.json`
# allowance; growing them past the baseline is a separate failure mode handled
# by the length check.
#
# Input protocol: tool payload arrives on stdin as JSON. Exit code 2 aborts
# the tool call. On success the payload is echoed back so downstream hooks
# (if any) receive the same input.

set -euo pipefail

payload=$(cat)

line_count=$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    data = json.loads(sys.stdin.read() or "{}")
except json.JSONDecodeError:
    print(0)
    sys.exit(0)
content = (data.get("tool_input") or {}).get("content", "") or ""
if not content:
    print(0)
else:
    print(content.count("\n") + (0 if content.endswith("\n") else 1))
')

if [ "${line_count:-0}" -gt 600 ]; then
    printf '[guard] BLOCKED: Write exceeds 600 lines (%s lines).\n' "$line_count" >&2
    printf '[guard] Hard cap matches the file-length baseline. Split into smaller modules.\n' >&2
    printf '[guard] See CLAUDE.md "Local QA" and scripts/qa/file_length_baseline.json.\n' >&2
    exit 2
fi

# Pass payload through for any chained hooks.
printf '%s' "$payload"
