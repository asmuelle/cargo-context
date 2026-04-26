# cargo-context
*High-Fidelity Context Engineering for Rust AI Workflows*

[![CI](https://github.com/asmuelle/cargo-context/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/asmuelle/cargo-context/actions/workflows/ci.yml)
[![Release](https://github.com/asmuelle/cargo-context/actions/workflows/release.yml/badge.svg)](https://github.com/asmuelle/cargo-context/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/rustc-1.95%2B-blue.svg)](https://blog.rust-lang.org/category/Releases/)

## Install

```bash
# Build from source (Rust 1.95+, edition 2024)
git clone https://github.com/asmuelle/cargo-context
cd cargo-context
cargo install --path crates/cargo-context-cli
cargo install --path crates/cargo-context-mcp
```

Pre-built binaries for Linux (x86_64, aarch64), macOS (x86_64, aarch64), and Windows ship from each tagged release on the [Releases page](https://github.com/asmuelle/cargo-context/releases).

Optional companions:
- `cargo install cargo-expand` — enables `--expand-macros`
- A local HuggingFace `tokenizer.json` — enables exact `--hf-llama3-vocab` counting

## Status

What's implemented and shipping today:

| Surface | Status |
|:---|:---|
| **Collection** — git diff, cargo metadata, compiler errors, entry points (syn-based signature extraction), related tests (integration + inline `#[cfg(test)]`) | ✅ |
| **Token budget** — Priority / Proportional / Truncate strategies; user prompt always exempt | ✅ |
| **Tokenizers** — `tiktoken-rs` for GPT/Claude (exact); calibrated heuristic for llama; `HfLlama3 { vocab_path }` for any local HF `tokenizer.json` | ✅ |
| **Scrubber** — built-in regex (AWS/GitHub/OpenAI/Anthropic/HF/Google/Slack/JWT/PEM); Shannon-entropy detection; path-globs (`.env`, `*.pem`, …); allowlist; YAML config; JSONL audit log | ✅ |
| **Macro expansion** — `cargo-expand` shell-out with `(path, mtime, lockhash)` cache | ✅ |
| **MCP server** — `cargo-context-mcp` binary on the official `rmcp` SDK; four tools | ✅ |
| **CLI** — `cargo context [pack flags]` and `cargo context scrub --check`; `--files-from <PATH\|->` and `--impact-scope <PATH\|->` for cargo-impact interop; `--scrub-report` / `--strict-scrub` for CI | ✅ |
| **Output formats** — markdown / xml / json / plain | ✅ |
| **MCP resources & prompts** — diff/errors/map resources and `fix_compiler_error` prompt | ✅ |
| **`--impact-scope` JSON envelope** ([#5](https://github.com/asmuelle/cargo-context/issues/5)) — confidence-sorted Scoped Files, `--min-confidence`, `--per-finding`, `--exclude-ids`, kind-aware language hints | ✅ |

## 1. Core Philosophy
The tool operates on the principle of **Signal-to-Noise Optimization (SNO)**. An LLM does not need your entire `src/` directory; it needs:
1. **The Symptom:** What is broken? (Recent errors)
2. **The Intent:** What is changing? (`git diff`)
3. **The Map:** How is the project structured? (`cargo metadata`)
4. **The Skeleton:** Where does the execution start? (Entry points)
5. **The Boundary:** How do we verify success? (Test targets)

---

## 2. Technical Feature Set

### A. The "Symptom" Capture (Error Integration)
`cargo-context` captures the current compiler state from `cargo check`.
*   **Mechanism:** It consumes Cargo's JSON diagnostic stream and extracts compiler messages.
*   **Contextualization:** It doesn't just dump the error; it extracts the specific file paths and line numbers mentioned in the error and automatically adds those files to the context pack.
*   **Workflow:** Use `cargo context --fix` to combine diagnostics, diff, and related tests.

### B. The "Intent" Capture (Git Integration)
Instead of the whole file, it focuses on the *evolution*.
*   **Mechanism:** Parses `git diff` (staged and unstaged).
*   **Vibe Optimization:** If a file has a massive diff, it provides a "summary" of the file and the specific changed chunks rather than the whole file, preventing token overflow.
*   **Workflow:** The CLI captures the working tree diff against `HEAD`; the MCP `get_diff` tool also accepts an optional ref range.

### C. The "Map" Capture (Metadata Integration)
Provides the AI with the architectural constraints.
*   **Mechanism:** Calls `cargo metadata --format-version 1`.
*   **Extraction:** Extracts the `Cargo.toml` dependencies, feature flags, and the workspace member tree.
*   **Purpose:** Tells the AI: *"You are working in a workspace with 3 crates; the current crate depends on `tokio` and `serde`."*

### D. The "Skeleton" Capture (Entry Points)
Prevents the AI from hallucinating where the logic begins.
*   **Mechanism:** Identifies `main.rs`, `lib.rs`, and any files marked with `#[tokio::main]` or similar attributes.
*   **Smart-Sampling:** It extracts the signatures of public functions/structs from these files rather than the full implementation (unless specifically requested).

### E. The "Boundary" Capture (Test Targets)
Ensures the AI writes code that is actually testable.
*   **Mechanism:** Scans for `#[cfg(test)]` modules and `tests/*.rs` files.
*   **Linkage:** If the `git diff` touches `src/auth.rs`, `cargo-context` automatically pulls in `tests/auth_tests.rs`.

---

## 3. CLI Interface (UX)

### 3.1 Common workflows

```bash
# Default — assemble a pack from whatever's relevant in cwd
cargo context

# "I'm fixing a bug" preset: errors + diff + related tests
cargo context --fix

# "I'm building a feature" preset: map + diff + entry points + related tests
cargo context --feature

# Pipe to a clipboard
cargo context --fix | pbcopy          # macOS
cargo context --fix | wl-copy          # Wayland

# Ask a question alongside the pack (stdin → "📝 User Prompt" section)
echo "why does verify_token return None?" | cargo context --fix

# Scope to specific files (cargo-impact interop)
cargo impact --context | cargo context --files-from -

# Consume a structured cargo-impact envelope — each finding becomes a
# Scoped File ordered by confidence desc, with per-file headers surfacing
# id / kind / severity / tier / confidence and kind-aware language hints.
cargo impact --format=json | cargo context --impact-scope -

# Filter by confidence, skip already-verified findings, or iterate one
# finding at a time.
cargo context --impact-scope impact.json --min-confidence 0.8
cargo context --impact-scope impact.json --exclude-ids f-aaaa,f-bbbb
cargo context --impact-scope impact.json --per-finding

# Compare a specific Git range instead of the working tree against HEAD.
cargo context --fix --diff HEAD~3..HEAD

# Validate the scrubber config without building a pack
cargo context scrub --check
```

### 3.2 Pack flags

| Flag | Default | Effect |
| :--- | :--- | :--- |
| `--preset <fix\|feature\|custom>` | `custom` | Selects which collectors run. `--fix` and `--feature` are shorthands. |
| `--max-tokens <N>` | `8000` | Hard ceiling on assembled pack size. |
| `--reserve-tokens <N>` | `2000` | Subtracted from `--max-tokens`; budget for the model's response. |
| `--budget-strategy <priority\|proportional\|truncate>` | `priority` | How to reconcile candidates with the token limit. |
| `--tokenizer <…>` | `llama3` | `llama3` / `llama2` / `tiktoken-cl100k` / `tiktoken-o200k` / `claude` / `chars-div4`. |
| `--hf-llama3-vocab <PATH>` | — | Exact counting via a local HuggingFace `tokenizer.json`. Overrides `--tokenizer`. |
| `--expand-macros <off\|auto\|on>` | `off` | Run `cargo-expand` and include expanded source. Auto fires when the diff has `.rs` files. |
| `--diff <RANGE>` | `HEAD` working tree | Use an explicit Git diff range, e.g. `HEAD~3..HEAD`. |
| `-f, --format <markdown\|xml\|json\|plain>` | `markdown` | Output format. Use `xml` for Claude, `json` for programmatic consumers. |
| `--files-from <PATH\|->` | — | Newline-delimited repo-relative paths to embed in a "📂 Scoped Files" section. `-` reads stdin. |
| `--impact-scope <PATH\|->` | — | Consume a `cargo-impact --format=json` envelope. Findings are filtered, sorted by confidence desc, and rendered as a "📂 Scoped Files" section. `-` reads stdin. Conflicts with `--files-from`. |
| `--min-confidence <F>` | — | Drop findings whose confidence is below `F` (range `[0.0, 1.0]`). Findings with no confidence field survive. Requires `--impact-scope`. |
| `--per-finding` | off | Emit one `📂 Impact: <id>` section per finding (with evidence + suggested action) instead of a single aggregated section. Requires `--impact-scope`. |
| `--exclude-ids <IDS>` | — | Comma-separated finding ids to skip (e.g. `f-aaaa,f-bbbb`). Requires `--impact-scope`. |
| `--include-path <GLOB>` | — | Force-include matching files in a separate "📌 Included Paths" section (repeatable). |
| `--exclude-path <GLOB>` | — | Suppress matching files from diff/scoped/impact/entry/expanded/test context. Exclude wins over include. |

### 3.3 Scrubber flags

| Flag | Effect |
| :--- | :--- |
| `--no-scrub` | Disable secret scrubbing entirely. Requires `--i-know-what-im-doing`. |
| `--scrub-report` | Print a per-category summary to stderr, e.g. `[scrub] 4 redacted (aws_key:3, jwt:1)`. |
| `--strict-scrub` | Exit `2` if any redaction occurred. CI-friendly tripwire. |

### 3.4 Subcommand: `cargo context scrub --check`

Validate `.cargo-context/scrub.yaml` (or any path passed via `--config`) and print a summary of the effective rule set:

```text
$ cargo context scrub --check
✓ .cargo-context/scrub.yaml v1 parsed

Effective rules:
  10 built-in pattern(s) active
  2 custom pattern(s) loaded

Entropy detection:
  enabled (min_length=20, threshold=4.5, 6 context key(s))

Paths:
  redact_whole: 7 glob(s)
  exclude:      1 glob(s)

Allowlist: 2 entries (0 exact, 2 regex)
```

Parse errors and invalid globs exit `1` with `✗ <path>:<line>:<col> — <message>` on stderr — suitable for a pre-commit hook.

---

## 4. The Output Payload (The "Context Pack")

The output is a structured document. `cargo-context` is **model-agnostic** — it does not call any LLM API. It produces a pack that any downstream tool (Claude, GPT, a local `llama.cpp` server, Ollama, `vLLM`, `mistral.rs`) can consume.

### 4.1 Output Formats

| Format | When to use | Notes |
| :--- | :--- | :--- |
| `markdown` | Default; copy-paste into any chat UI | Human-readable, works everywhere |
| `xml` | Claude-family models | Claude's attention benchmarks measurably better on `<file>`, `<diff>`, `<error>` tags |
| `json` | Programmatic consumers (MCP, scripts) | Schema versioned via `"schema": "cargo-context/v1"` |
| `plain` | Raw concatenation | No structural markers — for models that dislike markup noise |

Format selection is orthogonal to model choice; the same assembled content can be rendered as markdown, XML, JSON, or plain text.

### 4.2 Example Output Structure (markdown)

```markdown
# PROJECT CONTEXT PACK: [Project Name]
<!-- schema: cargo-context/v1 | tokens: 3842/8000 | tokenizer: llama3 -->

## 🗺️ Project Map
- Workspace: [Member A, Member B]
- Key Dependencies: tokio, axum, sqlx
- Entry Point: src/main.rs

## 🚨 Current State (Errors)
`error[E0308]: mismatched types` in `src/db.rs:42`
(Attached: src/db.rs [lines 30-50])

## ⚡ Intent (Git Diff)
Modified `src/auth.rs`:
- Added `verify_token` function.
- Updated `User` struct.

## 📂 Relevant Source Code
--- FILE: src/auth.rs ---
[Full or partial code based on diff]

--- FILE: src/db.rs ---
[Full or partial code based on error]
```

---

## 5. Token Budgeting

Token budgeting is a **first-class concern**, not an afterthought. The effective content budget is `--max-tokens - --reserve-tokens`; the user prompt section is exempt so the caller's actual question is never dropped.

### 5.1 Tokenizer Selection

`cargo-context` ships with pluggable tokenizers so counts match the *actual* downstream model:

| Tokenizer | Backing library | Default for |
| :--- | :--- | :--- |
| `llama3` | calibrated heuristic (~3.5 chars/token) | Local llama, Ollama |
| `llama2` | calibrated heuristic (~3.3 chars/token) | Legacy llama, CodeLlama |
| `tiktoken-cl100k` | `tiktoken-rs` | GPT-4, GPT-4o |
| `tiktoken-o200k` | `tiktoken-rs` | GPT-5 family |
| `claude` | `tiktoken-rs` `cl100k_base` approximation | Claude 3.5+ |
| `chars-div-4` | built-in heuristic | Unknown / offline fallback |
| `hf-llama3` | local HuggingFace `tokenizer.json` via `--hf-llama3-vocab` | Exact local vocab counting |

No tokenizer vocab is downloaded automatically. Use `--hf-llama3-vocab <PATH>` when exact HuggingFace tokenizer counting is required.

### 5.2 Budget Flags

| Flag | Default | Description |
| :--- | :--- | :--- |
| `--max-tokens <N>` | `8000` | Total model budget before reserve is subtracted. |
| `--tokenizer <name>` | `llama3` | One of the tokenizers above. |
| `--reserve-tokens <N>` | `2000` | Reserved for the model's response; subtracted from `--max-tokens`. |
| `--budget-strategy` | `priority` | `priority` (below), `proportional`, or `truncate`. |

### 5.3 Allocation Strategy

When `--budget-strategy=priority` (default), sections are packed in this order and later sections are dropped whole if they don't fit:

1. **User prompt** — exempt from budget pressure.
2. **Errors** — compiler diagnostics.
3. **Diff / scoped files / forced includes** — working tree intent and explicit user scope.
4. **Map** — workspace members and key dependencies.
5. **Entry points / expanded macros** — crate skeleton and optional expansion output.
6. **Related tests** — plausible verification targets.

Other strategies:

- `proportional` scales competing sections down by the same ratio and emits truncation markers.
- `truncate` keeps priority order, truncates the first overflowing section, then stops.

The final pack header reports actual tokens used, effective budget, tokenizer, and dropped sections:

```
<!-- schema: cargo-context/v1 | tokens: 5823/6000 | tokenizer: llama3 | dropped: 🎯 Related Tests -->
```

---

## 6. Macro Expansion

Rust's hardest AI-coding problem is opaque macros. `#[derive(Serialize)]`, `tokio::main`, `sqlx::query!`, and `#[actix_web::get]` hide the actual code the compiler sees. Without expansion, LLMs hallucinate behavior.

### 6.1 Mechanism

`cargo-context` shells out to `cargo expand` when:

- `--expand-macros=on` is passed, OR
- `--expand-macros=auto` is passed and the current diff contains Rust files.

### 6.2 Flags

| Flag | Description |
| :--- | :--- |
| `--expand-macros=on` | Expand macros in workspace members. |
| `--expand-macros=auto` | Expand only when the current diff contains `.rs` files. |
| `--expand-macros=off` | Never expand. |

### 6.3 Output Shape

Expanded code is included as a *sibling* section, not a replacement — the original is preserved for human readability:

```markdown
--- FILE: src/handlers.rs ---
#[tokio::main]
async fn main() { ... }

--- EXPANDED: src/handlers.rs ---
fn main() {
    let body = async { ... };
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(body)
}
```

### 6.4 Requirements & Fallbacks

- Requires `cargo-expand` on `PATH`. If missing or expansion fails, pack construction proceeds without expansion.
- Expansion output is cached by `(file_path, mtime, cargo_lock_hash)` in `target/cargo-context/expand/` so repeat runs are free.

---

## 7. Secret Scrubbing

`cargo-context` is designed to feed clipboards, chat UIs, and network-connected model APIs. A single unredacted `.env` diff can leak prod credentials. Scrubbing is **on by default** and runs *after* content assembly, *before* output.

### 7.1 Detection Layers

Three layers run in sequence; any match triggers redaction:

1. **Pattern-based** — compiled regex set covering:
   - AWS access keys (`AKIA[0-9A-Z]{16}`), AWS secrets
   - GitHub / GitLab tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `glpat-`)
   - OpenAI / Anthropic / HuggingFace keys (`sk-`, `sk-ant-`, `hf_`)
   - Google API keys (`AIza[0-9A-Za-z\-_]{35}`)
   - Slack tokens (`xox[baprs]-`)
   - Private keys (`-----BEGIN (RSA|EC|OPENSSH|PGP) PRIVATE KEY-----`)
   - JWTs (`eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+`)
   - Generic high-entropy strings in `KEY=`, `SECRET=`, `TOKEN=`, `PASSWORD=` assignments
2. **Entropy-based** — Shannon entropy > 4.5 on tokens ≥ 20 chars in values adjacent to suspicious keys. Catches rotated keys the pattern layer misses.
3. **Path-based** — project config can redact whole files by glob. `--include-path` can force a file into the pack, but scrub path redaction still applies. `--exclude-path` suppresses matching files before rendering.

### 7.2 Redaction Format

Matches are replaced with a stable fingerprint so the LLM still sees *structure* but not *value*:

```
AWS_SECRET_ACCESS_KEY=<REDACTED:aws_secret:8f2a>
```

The last 4 chars of a SHA-256 hash (`8f2a`) let humans correlate repeated occurrences without leaking the secret.

### 7.3 Flags

| Flag | Default | Description |
| :--- | :--- | :--- |
| `--no-scrub` | off | Disable all scrubbing. Requires `--i-know-what-im-doing`. |
| `--scrub-report` | off | Prints a summary of what was redacted (category + count, never value) to stderr. |
| `--strict-scrub` | off | Exit `2` after rendering if any redaction occurred. |

### 7.4 Guarantees

- CLI pack rendering scrubs candidate sections before budgeting/output.
- MCP `build_context_pack`, `get_diff`, `get_last_error`, `expand_macros`, resources, and prompts use the same workspace scrub config boundary.
- A non-zero exit code is returned if scrubbing had to redact anything and `--strict-scrub` is set — useful for CI pipes.
- The scrubber itself is open-source and its rule file lives at `.cargo-context/scrub.yaml` for project-level extension.

---

## 8. Integration Paths

`cargo-context` is designed to be a *data source*, not a chat client. Three integration modes are supported out of the box.

### 8.1 stdin/stdout (Unix Pipe)

Primary interface. The binary writes the pack to stdout; anything on stderr is diagnostics and safe to discard or capture separately.

```bash
# Pipe to clipboard
cargo context --fix | pbcopy          # macOS
cargo context --fix | wl-copy          # Wayland
cargo context --fix | xclip -sel clip  # X11

# Pipe directly to a local llama.cpp server
cargo context --fix --format=plain \
  | llama-cli --model ~/models/llama-3.1-8b-instruct.gguf --prompt-file /dev/stdin

# Pipe to Ollama
cargo context --fix --tokenizer=llama3 \
  | ollama run codellama

# Pipe to any OpenAI-compatible endpoint (vLLM, llama.cpp server, LM Studio)
cargo context --fix --format=json \
  | jq -Rs '{model:"local",messages:[{role:"user",content:.}]}' \
  | curl -s http://localhost:8080/v1/chat/completions \
      -H 'Content-Type: application/json' -d @-
```

`cargo-context` reads from stdin too — if data is piped in, it is prepended to the pack as a `## 📝 User Prompt` section. This lets editors and scripts supply the actual question:

```bash
echo "Why does verify_token return None for valid JWTs?" \
  | cargo context --fix \
  | llama-cli --model ~/models/llama-3.1-8b-instruct.gguf
```

### 8.2 MCP Server Mode

For agentic coding tools that speak the **Model Context Protocol** (Claude Code, Cursor, Continue, Zed AI), `cargo-context-mcp` runs as a long-lived stdio child process. Built on the official [`rmcp`](https://crates.io/crates/rmcp) SDK — full protocol compliance (initialize handshake, capability negotiation, structured content, JSON Schema for every tool's input).

```bash
# Spawn directly to verify
cargo-context-mcp
```

**Exposed tools:**

| Tool | Arguments | Returns |
| :--- | :--- | :--- |
| `build_context_pack` | `{ preset?, max_tokens?, reserve_tokens?, tokenizer?, budget_strategy? }` | Rendered markdown pack (string) |
| `get_last_error` | `{}` | Structured `Diagnostics` JSON (level, code, message, primary spans) |
| `get_diff` | `{ range?: "HEAD~3..HEAD" }` | Structured `Diff` JSON (FileDiff[] with hunks) |
| `expand_macros` | `{ file: string, crate_name: string }` | Expanded source via `cargo-expand` |

**Client config — Claude Code (`.claude/mcp.json` or `~/.config/claude-code/mcp.json`):**

```json
{
  "mcpServers": {
    "cargo-context": {
      "command": "cargo-context-mcp"
    }
  }
}
```

**Client config — Cursor / Continue / Zed:** same shape, just point at the binary on `PATH`. Working directory is whatever the client launches the server in; tools always operate on `std::env::current_dir()`.

Scrubbing, token budgeting, macro expansion, and `.cargo-context/scrub.yaml` auto-discovery all apply identically in MCP mode — the protocol boundary does not relax the guarantees. Diagnostics go to stderr via `tracing`; stdout stays a clean JSON-RPC channel.

### 8.3 Library Crate (`cargo_context_core`)

For tools that want to embed pack generation directly (custom CLIs, editor plugins, CI bots), the engine is published as a library:

```toml
[dependencies]
cargo-context-core = "0.1"
```

```rust
use cargo_context_core::{PackBuilder, Preset, Tokenizer};

let pack = PackBuilder::new()
    .preset(Preset::Fix)
    .max_tokens(8000)
    .tokenizer(Tokenizer::Llama3)
    .scrub(true)
    .build()?;

println!("{}", pack.render_markdown());
```

The CLI binary and MCP server are both thin shells over this crate.

---

## 9. Workspace Layout

`cargo-context` is a Cargo workspace of four crates. The split exists so embedding apps can depend on *just* the engine without pulling CLI argument parsers or MCP transports.

```text
cargo-context/
├── Cargo.toml                  # [workspace] manifest; shared deps/version
├── rust-toolchain.toml
├── crates/
│   ├── cargo-context-core/     # Pure library. No I/O to terminals, no network.
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── pack/
│   │       │   ├── mod.rs          # PackBuilder, Pack, Section
│   │       │   ├── render.rs       # section render helpers
│   │       │   └── impact.rs       # scoped files / cargo-impact rendering
│   │       ├── collect/
│   │       │   ├── mod.rs
│   │       │   ├── diff.rs         # git diff shell-out capture
│   │       │   ├── errors.rs       # cargo check JSON diagnostics
│   │       │   ├── meta.rs         # cargo_metadata wrapper
│   │       │   ├── entry.rs        # main.rs / lib.rs / #[*::main] discovery
│   │       │   └── tests.rs        # #[cfg(test)] + tests/ linkage
│   │       ├── expand.rs           # cargo-expand wrapper + cache
│   │       ├── tokenize.rs         # token counters and HF tokenizer cache
│   │       ├── budget.rs           # priority/proportional/truncate allocation
│   │       ├── scrub/
│   │       │   ├── mod.rs          # Scrubber pipeline
│   │       │   ├── patterns.rs     # built-in regex rules
│   │       │   ├── entropy.rs      # Shannon entropy detection
│   │       │   ├── paths.rs        # path-based rules
│   │       │   └── config.rs       # serde_yaml loader for scrub.yaml
│   │       └── error.rs            # thiserror enum
│   │   └── tests live inline beside the modules they exercise.
│   │
│   ├── cargo-context-cli/      # The `cargo context` subcommand binary.
│   │   ├── Cargo.toml              # [[bin]] name = "cargo-context"
│   │   └── src/
│   │       ├── main.rs             # clap parser; dispatches to core
│   │       ├── args.rs             # derive(Parser), presets (--fix, --feature)
│   │       └── tests/              # CLI integration tests
│   │
│   ├── cargo-context-mcp/      # MCP server binary.
│   │   ├── Cargo.toml              # [[bin]] name = "cargo-context-mcp"
│   │   └── src/
│   │       ├── main.rs             # stdio MCP process entry
│   │       ├── server.rs           # rmcp server bootstrap
│   │       └── tools.rs            # build_context_pack, get_last_error, get_diff, expand_macros
│   │
│   └── cargo-context-scrub/    # Extracted scrubber (standalone reuse).
│       ├── Cargo.toml
│       └── src/lib.rs              # Re-exports core::scrub for non-Rust-project use
│
├── scripts/qa/                 # local QA helpers
└── README.md
```

### 9.1 Dependency Graph

```text
cargo-context-cli ─┐
                   ├──> cargo-context-core ──> cargo-context-scrub
cargo-context-mcp ─┘                      └──> (tokenizers, tiktoken-rs, git, cargo_metadata)
```

The `core` crate has **zero** async runtime dependency — `tokio` lives only in `-mcp`. This keeps embedding in sync codebases (editor plugins, build scripts) cheap.

### 9.2 Key Crate Dependencies

| Crate | Depends on | Why |
| :--- | :--- | :--- |
| `core` | `cargo_metadata`, `tokenizers`, `tiktoken-rs`, `regex`, `globset`, `serde`, `serde_yaml`, `sha2`, `thiserror` | Pack assembly, tokenization, scrubbing |
| `cli` | `core`, `clap` (derive), `anyhow` | Argument parsing, TTY/stdin handling |
| `mcp` | `core`, `rmcp`, `tokio`, `tracing`, `schemars` | MCP stdio transport |
| `scrub` | `core::scrub` (re-export) | Allow non-cargo projects to reuse the scrubber |

### 9.3 MSRV & Release

- **MSRV:** Rust 1.95 (stable). `core` compiles on stable; macro expansion *shells out* to the user's `cargo-expand`, so `core` itself never needs nightly.
- **Release:** all four crates are versioned in lockstep from the workspace package version. Release automation builds binaries from tagged versions.

---

## 10. Scrub Rule Schema (`scrub.yaml`)

The scrubber's built-in rules live in `core` and are active by default. Projects extend or override them via `.cargo-context/scrub.yaml`, loaded automatically from the workspace root.

### 10.1 Top-Level Shape

```yaml
# .cargo-context/scrub.yaml
version: 1

# Merge mode for built-in rules:
#   "extend" (default) — add these to the built-in set
#   "replace"          — ignore built-ins entirely (dangerous)
#   "disable"          — turn off named built-ins, keep the rest
builtins: extend

# Individual built-in rules can be toggled by id.
disable_builtins:
  - jwt          # project intentionally shares JWTs in test fixtures

# Custom patterns added on top of built-ins.
patterns:
  - id: internal_api_key
    description: "ACME Corp internal API keys"
    regex: 'ACME_[A-Z0-9]{32}'
    category: api_key
    replacement: "<REDACTED:acme:{hash4}>"
    severity: high

  - id: stripe_live_key
    regex: 'sk_live_[A-Za-z0-9]{24,}'
    category: payment
    severity: critical

# Entropy-based detection tuning.
entropy:
  enabled: true
  min_length: 20        # only consider tokens this long
  threshold: 4.5        # Shannon entropy; higher = more random
  context_keys:         # only scan values next to these key names
    - key
    - secret
    - token
    - password
    - credential
    - api[_-]?key

# Path-based rules. Matched paths are redacted whole.
paths:
  redact_whole:
    - "**/.env"
    - "**/.env.*"
    - "**/*.pem"
    - "**/*.key"
    - "**/id_rsa*"
    - "**/credentials*"
    - "**/secrets/**"
  exclude:              # paths the scrubber should never touch
    - "**/test_fixtures/public_keys/**"
    - "docs/examples/demo.env"

# Allowlist: exact strings or regex that must NOT be redacted,
# even if another rule would match. Useful for public demo keys.
allowlist:
  - exact: "sk-ant-api03-PUBLIC-DEMO-KEY-DO-NOT-USE"
  - regex: '^AKIAEXAMPLE[0-9]+$'
  - regex: '^ghp_000000000000000000000000000000000000$'

# Reporting behavior.
report:
  log_file: null        # optional path; JSON lines of redactions (no values)
  max_entries: null     # optional cap for retained JSONL entries
```

### 10.2 Field Reference

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `version` | int | — | Schema version. Required. Current: `1`. |
| `builtins` | enum | `extend` | `extend` / `replace` / `disable`. |
| `disable_builtins` | string[] | `[]` | IDs of built-in rules to turn off. See §10.3. |
| `patterns[].id` | string | — | Stable identifier. Appears in redaction fingerprint and reports. |
| `patterns[].regex` | string | — | Rust `regex` crate syntax. Compiled at startup; invalid patterns fail loudly. |
| `patterns[].category` | string | `generic` | Free-form tag. Used in reports and replacement tokens. |
| `patterns[].replacement` | string | auto | Template. Placeholder: `{hash4}`. |
| `patterns[].severity` | enum | `high` | `low` / `medium` / `high` / `critical`. Affects `--strict-scrub` behavior. |
| `entropy.enabled` | bool | `true` | Master switch for entropy detection. |
| `entropy.min_length` | int | `20` | Tokens shorter than this are skipped. |
| `entropy.threshold` | float | `4.5` | Shannon entropy (bits). Range `0.0`–`8.0`. |
| `entropy.context_keys` | regex[] | built-in list | Key-name patterns that make adjacent values candidates. |
| `paths.redact_whole` | glob[] | built-in list | Files matched are replaced with `[REDACTED FILE: <path>]`. |
| `paths.exclude` | glob[] | `[]` | Paths exempted from *all* scrubbing. |
| `allowlist[].exact` | string | — | Literal string never redacted. |
| `allowlist[].regex` | string | — | Regex of strings never redacted. |
| `report.log_file` | path? | `null` | JSON-lines audit log. Never contains secret values — only rule metadata and `hash4`. |
| `report.max_entries` | int? | `null` | Retain only the most recent N audit log entries after each write. |

### 10.3 Built-In Rule IDs

Stable IDs users can `disable_builtins` without writing their own regex:

`aws_access_key`, `aws_secret_key`, `github_pat`, `github_oauth`, `gitlab_pat`,
`openai_key`, `anthropic_key`, `huggingface_token`, `google_api_key`,
`slack_token`, `stripe_key`, `private_key_pem`, `ssh_private_key`, `jwt`,
`generic_env_assignment`, `basic_auth_url`, `connection_string`.

Each built-in has the same shape as a user-defined pattern, so overriding one by ID (`disable_builtins: [jwt]` + a custom `patterns[].id: jwt`) is the canonical way to tighten a rule without forking the schema.

### 10.4 Validation

- `cargo context scrub --check` loads, validates, and prints effective rules (merged built-ins + overrides) without generating a pack. Useful for pre-commit hooks.
- Regex patterns are compiled once at startup; a malformed regex aborts with a line/column-anchored error rather than silently skipping.

---

## 11. Non-Goals

- **Not a chat client.** `cargo-context` never calls an LLM.
- **Not a code indexer.** No persistent AST graph, no cross-repo symbol search.
- **Not a prompt library.** Prompts live in your editor, agent, or shell.
- **Not opinionated about models.** Llama, Claude, GPT, Mistral, local or hosted — all equally supported.
