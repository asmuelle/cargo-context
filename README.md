# cargo-context
*High-Fidelity Context Engineering for Rust AI Workflows*

[![CI](https://github.com/asmuelle/cargo-context/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/asmuelle/cargo-context/actions/workflows/ci.yml)
[![Release](https://github.com/asmuelle/cargo-context/actions/workflows/release.yml/badge.svg)](https://github.com/asmuelle/cargo-context/actions/workflows/release.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/rustc-1.85%2B-blue.svg)](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html)

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
`cargo-context` will wrap `cargo check` or `cargo test`.
*   **Mechanism:** It captures `stderr` from the compiler.
*   **Contextualization:** It doesn't just dump the error; it extracts the specific file paths and line numbers mentioned in the error and automatically adds those files to the context pack.
*   **Flag:** `--last-error` (Captures the most recent failed build output).

### B. The "Intent" Capture (Git Integration)
Instead of the whole file, it focuses on the *evolution*.
*   **Mechanism:** Parses `git diff` (staged and unstaged).
*   **Vibe Optimization:** If a file has a massive diff, it provides a "summary" of the file and the specific changed chunks rather than the whole file, preventing token overflow.
*   **Flag:** `--diff [branch/commit]` (Specify the delta range).

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

```bash
# Basic usage: Generate a pack for the current state
cargo context

# "I'm fixing a bug" mode: Diff + Last Error + Related Tests
cargo context --fix

# "I'm building a feature" mode: Metadata + Entry points + Diff
cargo context --feature

# Pipe directly to a clipboard tool for immediate LLM pasting
cargo context --fix | pbcopy
```

### Command Flags:
| Flag | Description | Effect |
| :--- | :--- | :--- |
| `-d, --diff` | Include git changes | Adds `git diff` output to the pack. |
| `-e, --errors` | Include compiler errors | Runs `cargo check` and captures stderr. |
| `-m, --meta` | Include project map | Adds `Cargo.toml` and workspace structure. |
| `-t, --tests` | Include relevant tests | Finds tests that reference changed files. |
| `-f, --format` | Output format | `markdown` (default), `json`, or `xml`. |
| `--deep` | Full file inclusion | Includes full source of all referenced files. |

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

Format selection is orthogonal to model choice. A flag `--target=<claude|openai|llama|generic>` picks tag style and section ordering, but the *content* is identical.

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

Token budgeting is a **first-class concern**, not an afterthought. The pack will never silently exceed its budget.

### 5.1 Tokenizer Selection

`cargo-context` ships with pluggable tokenizers so counts match the *actual* downstream model:

| Tokenizer | Backing library | Default for |
| :--- | :--- | :--- |
| `llama3` | `tokenizers` crate (HuggingFace) with `meta-llama/Meta-Llama-3-8B` vocab | Local llama, Ollama |
| `llama2` | `tokenizers` crate | Legacy llama, CodeLlama |
| `tiktoken-cl100k` | `tiktoken-rs` | GPT-4, GPT-4o |
| `tiktoken-o200k` | `tiktoken-rs` | GPT-5 family |
| `claude` | `tokenizers` crate with Anthropic BPE approximation | Claude 3.5+ |
| `chars-div-4` | built-in heuristic | Unknown / offline fallback |

Tokenizer vocabs are downloaded once to `~/.cache/cargo-context/tokenizers/` and cached. Offline mode (`--offline`) falls back to `chars-div-4` and warns.

### 5.2 Budget Flags

| Flag | Default | Description |
| :--- | :--- | :--- |
| `--max-tokens <N>` | `8000` | Hard ceiling. Pack construction stops before exceeding. |
| `--tokenizer <name>` | `llama3` | One of the tokenizers above. |
| `--reserve-tokens <N>` | `2000` | Reserved for the model's response; subtracted from `--max-tokens`. |
| `--budget-strategy` | `priority` | `priority` (below), `proportional`, or `truncate`. |

### 5.3 Allocation Strategy

When `--budget-strategy=priority` (default), sections are packed in this order and later sections are dropped whole if they don't fit:

1. **Errors** (compiler diagnostics) — 20% floor
2. **Diff** (git changes) — 30% floor
3. **Directly referenced files** (from errors + diff) — remainder
4. **Map** (workspace, deps) — 5% cap
5. **Entry points** (signatures only) — 10% cap
6. **Related tests** — whatever is left

Each file included shows a header:

```
--- FILE: src/auth.rs (tokens: 412, truncated: false) ---
```

If a single file exceeds its allocation, `cargo-context` includes only the functions/items referenced by the error or diff, plus a footer:

```
[... 340 tokens elided: 4 unreferenced items. Use --include-path to force inclusion.]
```

The final pack header always reports actual vs. budget:

```
<!-- tokens: 7823/8000 (97.8%) | sections: errors,diff,files,map | dropped: tests -->
```

---

## 6. Macro Expansion

Rust's hardest AI-coding problem is opaque macros. `#[derive(Serialize)]`, `tokio::main`, `sqlx::query!`, and `#[actix_web::get]` hide the actual code the compiler sees. Without expansion, LLMs hallucinate behavior.

### 6.1 Mechanism

`cargo-context` shells out to `cargo expand` (via the `cargo-expand` subcommand, a thin wrapper over `rustc -Zunpretty=expanded`) when:

- `--expand-macros` is passed explicitly, OR
- An error in the captured stderr points at a line inside a macro invocation, OR
- The `--fix` preset is active and any file in the diff contains a proc-macro attribute.

### 6.2 Flags

| Flag | Description |
| :--- | :--- |
| `--expand-macros` | Always expand macros in files pulled into the pack. |
| `--expand-macros=auto` | Expand only when heuristic triggers (default when available). |
| `--expand-macros=off` | Never expand. |
| `--expand-filter <regex>` | Expand only macros matching pattern (e.g. `^sqlx::`). |

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

- Requires `cargo-expand` on `PATH`. If missing, `cargo-context` prints a one-line install hint (`cargo install cargo-expand`) and proceeds without expansion rather than failing.
- Requires a nightly `rustc` for full fidelity. On stable-only systems, `cargo-context` falls back to `rustc --pretty=expanded` via `RUSTC_BOOTSTRAP=1` only if `--allow-bootstrap` is explicitly set; otherwise it skips with a warning.
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
3. **Path-based** — file paths matching `.env*`, `*.pem`, `*.key`, `id_rsa*`, `*credentials*`, `*secrets*` are redacted whole unless `--include-path` explicitly overrides.

### 7.2 Redaction Format

Matches are replaced with a stable fingerprint so the LLM still sees *structure* but not *value*:

```
AWS_SECRET_ACCESS_KEY=<REDACTED:aws_secret:8f2a>
```

The last 4 chars of a SHA-256 hash (`8f2a`) let humans correlate repeated occurrences without leaking the secret.

### 7.3 Flags

| Flag | Default | Description |
| :--- | :--- | :--- |
| `--scrub` | `on` | Master switch. `--scrub=off` disables (requires `--i-know-what-im-doing`). |
| `--scrub-rules <path>` | — | Extra YAML rule file merged with defaults. |
| `--scrub-allowlist <path>` | — | Known-safe patterns to skip (e.g. public demo keys). |
| `--scrub-report` | off | Prints a summary of what was redacted (category + count, never value) to stderr. |

### 7.4 Guarantees

- Scrubbing runs on the *final assembled pack*, so it catches secrets introduced by expansion, diff hunks, and error messages alike.
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

For agentic coding tools (Claude Code, Cursor, Continue, Zed AI) that speak the **Model Context Protocol**, `cargo-context` runs as a long-lived stdio or SSE server:

```bash
# stdio transport (default; what Claude Code / Cursor spawn)
cargo context mcp

# SSE transport on localhost (for browser-based clients)
cargo context mcp --transport=sse --port=7878
```

**Exposed MCP primitives:**

| Kind | Name | Arguments | Returns |
| :--- | :--- | :--- | :--- |
| `tool` | `build_context_pack` | `{ preset: "fix"\|"feature"\|"custom", max_tokens?, tokenizer?, include_paths?, exclude_paths? }` | Pack as text resource |
| `tool` | `get_last_error` | `{}` | Captured compiler diagnostics + referenced files |
| `tool` | `get_diff` | `{ range?: "HEAD~3..HEAD" }` | Scrubbed diff with file-level summaries |
| `tool` | `expand_macros` | `{ file: string, item?: string }` | Macro-expanded source |
| `resource` | `cargo-context://pack/current` | — | Live pack for the cwd, refreshed on read |
| `resource` | `cargo-context://map` | — | Workspace structure only (cheap, cacheable) |
| `prompt` | `fix_compiler_error` | `{}` | Pre-formatted prompt pairing pack + instruction |

**Client config example (Claude Code `.mcp.json`):**

```json
{
  "mcpServers": {
    "cargo-context": {
      "command": "cargo",
      "args": ["context", "mcp"],
      "env": { "CARGO_CONTEXT_MAX_TOKENS": "12000" }
    }
  }
}
```

Scrubbing, token budgeting, and macro expansion all apply identically in MCP mode — the protocol boundary does not relax the guarantees.

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
├── Cargo.toml                  # [workspace] manifest; pins rust-toolchain = "stable"
├── rust-toolchain.toml
├── crates/
│   ├── cargo-context-core/     # Pure library. No I/O to terminals, no network.
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── pack/
│   │       │   ├── mod.rs          # PackBuilder, Pack, Section
│   │       │   ├── render.rs       # markdown / xml / json / plain
│   │       │   └── schema.rs       # serde types for JSON output
│   │       ├── collect/
│   │       │   ├── mod.rs
│   │       │   ├── diff.rs         # git2-based diff capture
│   │       │   ├── errors.rs       # cargo check stderr parsing (cargo_metadata::Message)
│   │       │   ├── meta.rs         # cargo_metadata wrapper
│   │       │   ├── entry.rs        # main.rs / lib.rs / #[*::main] discovery
│   │       │   └── tests.rs        # #[cfg(test)] + tests/ linkage
│   │       ├── expand/
│   │       │   ├── mod.rs          # cargo-expand wrapper + cache
│   │       │   └── cache.rs        # target/cargo-context/expand/ keyed by (path, mtime, lockhash)
│   │       ├── tokenize/
│   │       │   ├── mod.rs          # Tokenizer trait
│   │       │   ├── llama.rs        # tokenizers crate, HF vocab
│   │       │   ├── tiktoken.rs     # tiktoken-rs
│   │       │   ├── claude.rs       # anthropic BPE approximation
│   │       │   └── heuristic.rs    # chars/4 fallback
│   │       ├── budget/
│   │       │   ├── mod.rs          # Budget, Allocation
│   │       │   └── priority.rs     # priority/proportional/truncate strategies
│   │       ├── scrub/
│   │       │   ├── mod.rs          # Scrubber pipeline
│   │       │   ├── patterns.rs     # built-in regex rules
│   │       │   ├── entropy.rs      # Shannon entropy detection
│   │       │   ├── paths.rs        # path-based rules
│   │       │   └── config.rs       # serde_yaml loader for scrub.yaml
│   │       └── error.rs            # thiserror enum
│   │   └── tests/
│   │       ├── pack_snapshot.rs    # insta snapshots of rendered packs
│   │       ├── scrub_property.rs   # proptest: no secret in output
│   │       └── budget_invariants.rs
│   │
│   ├── cargo-context-cli/      # The `cargo context` subcommand binary.
│   │   ├── Cargo.toml              # [[bin]] name = "cargo-context"
│   │   └── src/
│   │       ├── main.rs             # clap parser; dispatches to core
│   │       ├── args.rs             # derive(Parser), presets (--fix, --feature)
│   │       └── stdin.rs            # reads piped prompt, forwards to PackBuilder
│   │
│   ├── cargo-context-mcp/      # MCP server binary.
│   │   ├── Cargo.toml              # [[bin]] name = "cargo-context-mcp"
│   │   └── src/
│   │       ├── main.rs             # launched by `cargo context mcp`
│   │       ├── server.rs           # rmcp server bootstrap
│   │       ├── tools.rs            # build_context_pack, get_last_error, etc.
│   │       ├── resources.rs        # cargo-context://pack/current
│   │       └── prompts.rs          # fix_compiler_error
│   │
│   └── cargo-context-scrub/    # Extracted scrubber (standalone reuse).
│       ├── Cargo.toml
│       └── src/lib.rs              # Re-exports core::scrub for non-Rust-project use
│
├── xtask/                      # cargo-xtask for release, vocab download, snapshot updates
│   └── src/main.rs
├── .cargo-context/
│   └── scrub.yaml              # Project-level scrub rules (shipped as example)
└── examples/
    ├── pipe_to_ollama.sh
    ├── pipe_to_llama_cpp.sh
    └── mcp_claude_code.json
```

### 9.1 Dependency Graph

```text
cargo-context-cli ─┐
                   ├──> cargo-context-core ──> cargo-context-scrub
cargo-context-mcp ─┘                      └──> (tokenizers, tiktoken-rs, git2, cargo_metadata)
```

The `core` crate has **zero** async runtime dependency — `tokio` lives only in `-mcp`. This keeps embedding in sync codebases (editor plugins, build scripts) cheap.

### 9.2 Key Crate Dependencies

| Crate | Depends on | Why |
| :--- | :--- | :--- |
| `core` | `git2`, `cargo_metadata`, `tokenizers`, `tiktoken-rs`, `regex`, `serde`, `serde_yaml`, `sha2`, `thiserror`, `ignore` | Pack assembly, tokenization, scrubbing |
| `cli` | `core`, `clap` (derive), `anyhow`, `atty` | Argument parsing, TTY detection for color |
| `mcp` | `core`, `rmcp` (or `mcp-sdk`), `tokio`, `tracing` | MCP stdio/SSE transport |
| `scrub` | `core::scrub` (re-export) | Allow non-cargo projects to reuse the scrubber |

### 9.3 MSRV & Release

- **MSRV:** Rust 1.85 (stable). `core` compiles on stable; macro expansion *shells out* to the user's `cargo-expand`, so `core` itself never needs nightly.
- **Release:** `xtask release` cuts all four crates in lockstep with matching versions. `cargo-context-core` is semver-stable from `0.1`; binaries follow.

---

## 10. Scrub Rule Schema (`scrub.yaml`)

The scrubber's built-in rules live in `core` and are always active. Projects extend or override them via `.cargo-context/scrub.yaml` (loaded automatically from the workspace root) or any file passed to `--scrub-rules`.

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
  stderr_summary: true  # print `[scrub] redacted 4 secrets (aws:1, jwt:2, entropy:1)`
  fail_on_match: false  # CI mode: exit non-zero if anything was scrubbed
  log_file: null        # optional path; JSON lines of redactions (no values)
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
| `patterns[].replacement` | string | auto | Template. Placeholders: `{id}`, `{category}`, `{hash4}`, `{hash8}`. |
| `patterns[].severity` | enum | `high` | `low` / `medium` / `high` / `critical`. Affects `--strict-scrub` behavior. |
| `entropy.enabled` | bool | `true` | Master switch for entropy detection. |
| `entropy.min_length` | int | `20` | Tokens shorter than this are skipped. |
| `entropy.threshold` | float | `4.5` | Shannon entropy (bits). Range `0.0`–`8.0`. |
| `entropy.context_keys` | regex[] | built-in list | Key-name patterns that make adjacent values candidates. |
| `paths.redact_whole` | glob[] | built-in list | Files matched are replaced with `[REDACTED FILE: <path>]`. |
| `paths.exclude` | glob[] | `[]` | Paths exempted from *all* scrubbing. |
| `allowlist[].exact` | string | — | Literal string never redacted. |
| `allowlist[].regex` | string | — | Regex of strings never redacted. |
| `report.stderr_summary` | bool | `true` | One-line summary on stderr after pack generation. |
| `report.fail_on_match` | bool | `false` | Equivalent to always passing `--strict-scrub`. |
| `report.log_file` | path? | `null` | JSON-lines audit log. Never contains secret values — only `{id, category, file, line, hash4}`. |

### 10.3 Built-In Rule IDs

Stable IDs users can `disable_builtins` without writing their own regex:

`aws_access_key`, `aws_secret_key`, `github_pat`, `github_oauth`, `gitlab_pat`,
`openai_key`, `anthropic_key`, `huggingface_token`, `google_api_key`,
`slack_token`, `stripe_key`, `private_key_pem`, `ssh_private_key`, `jwt`,
`generic_env_assignment`, `basic_auth_url`, `connection_string`.

Each built-in has the same shape as a user-defined pattern, so overriding one by ID (`disable_builtins: [jwt]` + a custom `patterns[].id: jwt`) is the canonical way to tighten a rule without forking the schema.

### 10.4 Validation

- The file is validated against a JSON Schema shipped at `crates/cargo-context-core/schemas/scrub.v1.json`.
- `cargo context scrub --check` loads, validates, and prints effective rules (merged built-ins + overrides) without generating a pack. Useful for pre-commit hooks.
- Regex patterns are compiled once at startup; a malformed regex aborts with a line/column-anchored error rather than silently skipping.

---

## 11. Non-Goals

- **Not a chat client.** `cargo-context` never calls an LLM.
- **Not a code indexer.** No persistent AST graph, no cross-repo symbol search.
- **Not a prompt library.** Prompts live in your editor, agent, or shell.
- **Not opinionated about models.** Llama, Claude, GPT, Mistral, local or hosted — all equally supported.

