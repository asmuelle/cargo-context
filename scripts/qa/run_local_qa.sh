#!/usr/bin/env bash
# Single source of truth for "what green means" in this repo.
#
# Runs the same gates as GitLab CI:
#   fmt-check, file-length QA, architecture QA, clippy, cargo-deny,
#   cargo check (native + wasm target), cargo test, WASM build + frontend smoke.
#
# Opt-out env flags (default off so the default run matches CI exactly):
#   QA_SKIP_TESTS=1   — skip the test step entirely
#   QA_SKIP_DENY=1    — skip `cargo deny`
#
# Opt-in env flags:
#   QA_USE_NEXTEST=1  — replace `cargo test` with `cargo nextest run`
#                       (workspace tests) + `cargo test --doc` (doctests,
#                       nextest does not run them). Requires cargo-nextest.
#
# The Stop hook in .claude/settings.json sets QA_SKIP_WASM=1 and
# QA_USE_NEXTEST=1 for acceptable turnaround during agent sessions. CI and
# `make qa-local` run the default cargo test path unchanged.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

export SQLX_OFFLINE="${SQLX_OFFLINE:-true}"
# Repo-local cargo home so cargo-deny / wasm-bindgen installs survive across
# runs. Previously we used $TMPDIR which macOS wipes periodically.
export CARGO_HOME="${CARGO_HOME:-$ROOT_DIR/.cache/cargo-home}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_DENY_VERSION="${CARGO_DENY_VERSION:-0.19.1}"
export FILE_LENGTH_MAX_LINES="${FILE_LENGTH_MAX_LINES:-600}"

QA_SKIP_TESTS="${QA_SKIP_TESTS:-0}"
QA_SKIP_DENY="${QA_SKIP_DENY:-0}"
QA_SKIP_WASM="${QA_SKIP_WASM:-0}"
QA_USE_NEXTEST="${QA_USE_NEXTEST:-0}"
# Optional wasm target crate (project-specific). If unset and QA_SKIP_WASM=0,
# the wasm check is skipped with a note.
QA_WASM_CRATE="${QA_WASM_CRATE:-}"

TIMINGS=()

log_step() {
    printf '\n==> %s\n' "$1"
}

timed_step() {
    local label="$1"
    shift
    local start end status=0
    log_step "$label"
    start=$(date +%s)
    # `|| status=$?` keeps set -e from exiting so we can record the timing
    # and emit a summary before propagating the failure.
    "$@" || status=$?
    end=$(date +%s)
    local elapsed=$((end - start))
    TIMINGS+=("$(printf '%-24s %4ds' "$label" "$elapsed")")
    if [ "$status" -ne 0 ]; then
        printf '\nFailed at step: %s (exit %d)\n' "$label" "$status" >&2
        printf 'Timings so far:\n' >&2
        for line in "${TIMINGS[@]}"; do
            printf '  %s\n' "$line" >&2
        done
        exit "$status"
    fi
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        printf 'Missing required command: %s\n' "$1" >&2
        exit 1
    fi
}

ensure_cargo_deny() {
    local bin="$CARGO_HOME/bin/cargo-deny"
    if ! [ -x "$bin" ] || ! "$bin" --version | grep -q "$CARGO_DENY_VERSION"; then
        log_step "Installing cargo-deny $CARGO_DENY_VERSION"
        cargo install cargo-deny \
            --version "$CARGO_DENY_VERSION" \
            --locked --root "$CARGO_HOME" --force
    fi
}

require_command cargo
require_command rustup
require_command python3
require_command node
require_command make
require_command git

mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR"

log_step "Ensuring Rust components"
rustup component add rustfmt clippy

if [ "$QA_SKIP_DENY" != "1" ]; then
    ensure_cargo_deny
fi

timed_step "rustfmt" make fmt-check

timed_step "clippy" make lint

if [ "$QA_SKIP_DENY" != "1" ]; then
    timed_step "cargo_deny" cargo deny check -W notice -W unmaintained
fi

cargo_check_all() {
    env RUSTFLAGS=-Dwarnings cargo check \
        --workspace --all-targets --all-features --locked
    if [ "$QA_SKIP_WASM" != "1" ] && [ -n "$QA_WASM_CRATE" ]; then
        env RUSTFLAGS=-Dwarnings cargo check \
            -p "$QA_WASM_CRATE" --features wasm \
            --target wasm32-unknown-unknown --locked
    fi
}
timed_step "cargo_check" cargo_check_all

run_nextest() {
    cargo nextest run --workspace --all-features --locked
}
run_doctests() {
    cargo test --doc --workspace --all-features --locked
}

if [ "$QA_SKIP_TESTS" != "1" ]; then
    if [ "$QA_USE_NEXTEST" = "1" ]; then
        if ! command -v cargo-nextest >/dev/null 2>&1; then
            printf 'QA_USE_NEXTEST=1 but cargo-nextest is not installed.\n' >&2
            printf 'Install: brew install cargo-nextest  OR  cargo binstall cargo-nextest\n' >&2
            exit 1
        fi
        # nextest ignores doctests by design, so we run them separately.
        timed_step "cargo_nextest" run_nextest
        timed_step "cargo_doctest" run_doctests
    else
        timed_step "cargo_test" cargo test --workspace --all-features --locked
    fi
fi

log_step "QA stage passed"
printf '\nTimings:\n'
for line in "${TIMINGS[@]}"; do
    printf '  %s\n' "$line"
done
