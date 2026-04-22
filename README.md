# cargo-context
*High-Fidelity Context Engineering for Rust AI Workflows*

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

The output is formatted as a single, structured Markdown document optimized for LLMs (Claude 3.5 Sonnet / GPT-4o).

**Example Output Structure:**
```markdown
# PROJECT CONTEXT PACK: [Project Name]

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

