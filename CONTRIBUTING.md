# Contributing to cargo-context

Thanks for picking this up. This guide is short by design: follow the workflow,
keep the gates green, and ship small PRs.

## Quick start

```bash
git clone https://github.com/andreasmueller/cargo-context
cd cargo-context

# Build + run all local gates (mirrors CI).
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo install cargo-deny --locked && cargo deny check
```

If all five pass locally, CI will pass too.

## Workspace layout

```
crates/
├── cargo-context-core/    # Library: pack assembly, tokenize, budget, scrub, expand, collect
├── cargo-context-cli/     # Binary: `cargo-context` (Cargo subcommand)
├── cargo-context-mcp/     # Binary: `cargo-context-mcp` (MCP stdio server)
└── cargo-context-scrub/   # Re-export of core::scrub for non-Cargo codebases
```

Rule of thumb: new behavior lives in `core`. The other three crates stay thin.

## Development loop

1. **Pick an issue** — or file one first if the change is non-trivial, so scope can be agreed.
2. **Branch** — `feat/short-name`, `fix/short-name`, `docs/short-name`.
3. **Write the test first** — `#[cfg(test)]` module next to the code, or `tests/*.rs` for integration.
4. **Implement** — prefer small focused commits; conventional-commit style (`feat:`, `fix:`, `refactor:`, etc.).
5. **Run the gates** — see Quick start above.
6. **Open a PR** — the template will walk you through the checklist.

## Coding standards

- **Format:** `cargo fmt` (config in `rustfmt.toml`).
- **Lint:** `cargo clippy -- -D warnings` (config in `clippy.toml`, MSRV 1.85).
- **Errors:** `thiserror` in libraries, `anyhow` in binaries. No `unwrap()` / `expect()` in production paths.
- **Public API:** doc-commented with `///`. Rustdoc is built with `-D warnings` in CI.
- **Immutability first:** prefer new values over `&mut`; borrow by default.
- **Tests:** keep unit tests next to the code, use `rstest` for parameterized cases, `proptest` for properties.

## Adding a new scrubber rule

1. Add a built-in pattern to `crates/cargo-context-core/src/scrub.rs` with a stable `id`.
2. Add a test asserting the pattern redacts a realistic example **and** a test asserting a similar-looking benign string is *not* redacted.
3. Document the new `id` in README §10.3.
4. Flag the PR with the `security` label — CODEOWNERS will request review on scrubber changes automatically.

## Adding a new tokenizer

1. Add a variant to `Tokenizer` in `crates/cargo-context-core/src/tokenize.rs`.
2. Extend the `label()` and `count()` impls. If the tokenizer requires vocab download, implement a lazy-init cache under `~/.cache/cargo-context/tokenizers/`.
3. Wire it through `PackBuilder::tokenizer` (already generic) and the CLI's `TokenizerArg` enum.
4. Add a unit test covering at least one known-tokenization fixture.

## Adding a new MCP tool

1. Extend the `tools/list` response and the `handle_tool_call` match arm in `crates/cargo-context-mcp/src/main.rs`.
2. Add an `inputSchema` JSON Schema for the arguments.
3. Scrubbing and budgeting **must** apply before returning content — never relax those guarantees at the protocol boundary.

## Commit messages

Conventional style:

```
feat(core): add llama3 tokenizer
fix(mcp): escape stderr diagnostics so JSON-RPC channel stays clean
refactor(cli): extract preset parsing into helper
docs(readme): document scrub.yaml schema
chore(deps): bump serde to 1.0.215
```

No `Co-authored-by` trailers, no `Signed-off-by` required. Squash-merge is the default.

## Dependencies

- New runtime deps need a one-line justification in the PR (what it does, why a hand-rolled version isn't better, license).
- Licenses are allow-listed in `deny.toml`. Anything outside that list requires either a `deny.toml` exception with reasoning or a different crate.
- `cargo deny check` runs in CI; fix any `bans` or `sources` failures before merge.

## Release process

1. Bump workspace version in `Cargo.toml` (`[workspace.package] version = "x.y.z"`).
2. Update `CHANGELOG.md` (if/when one exists).
3. Tag: `git tag vx.y.z && git push --tags`.
4. The `release.yml` workflow builds cross-platform binaries and opens a GitHub release with auto-generated notes.
5. Publish to crates.io (maintainer only): `cargo publish -p cargo-context-core` → `-p cargo-context-scrub` → `-p cargo-context-cli` → `-p cargo-context-mcp`.

Prerelease tags (e.g. `v0.2.0-rc1`) are automatically marked `prerelease: true`.

## Security

- Never include real secrets in tests, fixtures, or PR descriptions — the scrubber's test vectors use `EXAMPLE` suffixes and `000000...` allowlist patterns.
- Security-sensitive reports: do **not** file a public issue. See `SECURITY.md` (or email the maintainer) for private disclosure.

## Scope

cargo-context is intentionally narrow. Before proposing a feature, re-read the **Non-Goals** section of the README. Requests that push the project toward being a chat client, a code indexer, or a prompt library will be closed as out-of-scope.

## License

By contributing, you agree that your contributions will be licensed under the project's Apache-2.0 license.
