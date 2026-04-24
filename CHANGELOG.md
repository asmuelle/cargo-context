# Changelog

All notable changes to this project are documented in this file. The format is
loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-04-24

### Added

- **`--impact-scope` now consumes the full `cargo-impact` schema** ([#5]).
  The envelope is parsed into typed `Finding` records (id, kind, confidence,
  severity, tier, evidence, suggested_action) rather than a flat path list.
- **Aggregated mode** renders a single "📂 Scoped Files" section with files
  ordered by confidence descending; each file header surfaces the finding's
  id, kind, severity/tier, and confidence. Co-located findings share one
  file block.
- **`--per-finding`** emits one `📂 Impact: <id>` section per finding, each
  containing the finding's evidence, suggested action, and primary file —
  useful when an agent wants to iterate through findings one at a time.
- **`--min-confidence <F>`** drops findings below the threshold. Findings
  with no confidence field survive (unknown ≠ below threshold). Range
  checked to `[0.0, 1.0]`.
- **`--exclude-ids f-aaaa,f-bbbb`** skips specific finding ids — useful for
  filtering out already-verified findings from subsequent packs.
- **`--impact-scope -`** reads the envelope from stdin (mutually exclusive
  with `--files-from -`).
- **Kind-aware language hints**: `doc_drift_link` / `doc_drift_keyword`
  findings render as `markdown` regardless of file extension.
- **Public API**: new `cargo_context_core::impact` module exporting
  `Finding`, `parse_envelope`, `filter_and_sort`, and `unique_paths`.
  `PackBuilder` gains `impact_findings(Vec<Finding>)` and
  `impact_per_finding(bool)`.

### Tests

- Workspace now runs 120 tests (was 107), all passing. Clippy
  `-D warnings` clean; `cargo fmt --check` clean.

[#5]: https://github.com/asmuelle/cargo-context/issues/5

## [0.1.1] - 2026-04-23

### Added

- SLSA build provenance attestations on release artifacts.
- MCP server gains resources and prompts (previously tools-only).
- Initial `--impact-scope <PATH>` — consumes a `cargo-impact --format=json`
  envelope and routes extracted paths through the Scoped Files section.
- MSRV pinned to Rust 1.95; workspace upgraded to edition 2024.

## [0.1.0] - Initial release

- Context pack assembly (diff, errors, metadata, entry points, related tests).
- Token budget strategies: Priority, Proportional, Truncate.
- Tokenizers: llama3/llama2 calibration, tiktoken, Claude, chars-div4,
  local HuggingFace `tokenizer.json`.
- Secret scrubber with built-in regex, entropy detection, path globs,
  allowlist, YAML config, JSONL audit log.
- Macro expansion via `cargo-expand` with `(path, mtime, lockhash)` cache.
- `cargo-context-mcp` binary on the official `rmcp` SDK.
- Output formats: markdown / xml / json / plain.
- `cargo context scrub --check` for YAML validation.
