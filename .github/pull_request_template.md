## Summary

<!-- One or two sentences: what changed and why. -->

## Changes

<!-- Bullet points of concrete changes. Link issues with "Closes #123". -->

-
-

## Test plan

<!-- What you ran locally, and what CI will verify. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo doc --workspace --no-deps` (if public API or doc comments changed)
- [ ] Manual smoke test of the affected path (describe below)

<!-- Describe manual verification, if any: -->

## Checklist

- [ ] Public API changes documented with `///` doc comments
- [ ] New deps justified in the PR description (why this crate, license, alternatives considered)
- [ ] Secret-scrubber rules or new integration points reviewed for leakage risk
- [ ] No `unwrap()` / `expect()` in production code paths (tests are fine)
- [ ] `Cargo.lock` updated intentionally (or unchanged)
- [ ] Breaking changes called out in the summary
