#!/usr/bin/env bash
# PostToolUse hook runner. Called after every Write/Edit.
#
# Fast, per-edit feedback only. Heavy checks (clippy, tests, WASM, architecture,
# cargo-deny) run via scripts/qa/run_local_qa.sh at session end (Stop hook)
# or before pushing.
#
# Opt-in env flag:
#   POST_EDIT_CLIPPY=1  — run scoped `cargo clippy` instead of `cargo check`.
#     Matches `make lint`'s deny flags on the edited crate only. Catches
#     unwrap_used / expect_used violations at edit time instead of at Stop.
#     The agent-session Claude settings enable this by default; CI and
#     manual git operations do not.
#
# Usage: scripts/qa/post_edit.sh <FILE_PATH>

set -euo pipefail

FILE_PATH="${1:-${FILE_PATH:-}}"
if [ -z "$FILE_PATH" ]; then
    exit 0
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

is_rust=false
is_frontend=false
case "$FILE_PATH" in
    *.rs)                                  is_rust=true ;;
    *.js|*.ts|*.tsx|*.jsx|*.css|*.html)    is_frontend=true ;;
esac

if $is_rust; then
    # Format only the edited file. Cleaner diffs than `cargo fmt --all` and
    # avoids re-touching files Claude did not modify.
    if command -v rustfmt >/dev/null 2>&1; then
        rustfmt --edition 2021 "$FILE_PATH" >/dev/null 2>&1 || true
    fi

    # Per-crate `cargo check` is ~5x faster than workspace clippy. Full clippy
    # runs in run_local_qa.sh at session end.
    #
    # Convention: each `crates/<name>/` maps to `-p <name>`. Files outside
    # that layout skip the scoped check.
    crate_flag=""
    case "$FILE_PATH" in
        crates/*)
            crate_name="${FILE_PATH#crates/}"
            crate_name="${crate_name%%/*}"
            if [ -n "$crate_name" ]; then
                crate_flag="-p $crate_name"
            fi
            ;;
    esac

    if [ -n "$crate_flag" ]; then
        # pipefail preserves cargo's exit code through `sed`. We cap output
        # instead of using `head`, which would send SIGPIPE and risk masking
        # the real status on some shells.
        if [ "${POST_EDIT_CLIPPY:-0}" = "1" ]; then
            # Scoped clippy matches `make lint` so the deny set (correctness,
            # suspicious, perf, plus unwrap/expect on lib+bins) is enforced
            # at edit time on just the edited crate.
            # shellcheck disable=SC2086
            SQLX_OFFLINE=true cargo clippy $crate_flag --all-targets --all-features --locked \
                -- -D clippy::correctness -D clippy::suspicious -D clippy::perf 2>&1 \
                | sed -n '1,60p'
            # shellcheck disable=SC2086
            SQLX_OFFLINE=true cargo clippy $crate_flag --lib --bins --all-features --locked \
                -- -D clippy::unwrap_used -D clippy::expect_used 2>&1 \
                | sed -n '1,60p'
        else
            # shellcheck disable=SC2086
            SQLX_OFFLINE=true cargo check $crate_flag --all-targets --locked 2>&1 \
                | sed -n '1,60p'
        fi
    fi
fi

if $is_rust || $is_frontend; then
    # File-length guard (optional: only runs if the helper exists).
    if [ -x scripts/qa/check_file_lengths.py ] \
        || [ -f scripts/qa/check_file_lengths.py ]; then
        python3 scripts/qa/check_file_lengths.py \
            --max-lines 600 \
            --baseline scripts/qa/file_length_baseline.json
    fi
fi
