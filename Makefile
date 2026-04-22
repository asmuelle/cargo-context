# Developer shortcuts for the cargo-context workspace.
#
# Keep targets small and composable; scripts/qa/run_local_qa.sh is the
# single source of truth for the full CI gate sequence.

CARGO ?= cargo
WORKSPACE_FLAGS := --workspace --all-targets --locked

.PHONY: help fmt fmt-check lint lint-fix test doc deny qa-local clean

help:
	@echo "Targets:"
	@echo "  fmt         — apply rustfmt"
	@echo "  fmt-check   — verify rustfmt (CI gate)"
	@echo "  lint        — clippy with -D warnings (CI gate)"
	@echo "  lint-fix    — clippy --fix"
	@echo "  test        — cargo test --workspace"
	@echo "  doc         — rustdoc with -D warnings"
	@echo "  deny        — cargo deny check"
	@echo "  qa-local    — full local CI-equivalent gate"
	@echo "  clean       — cargo clean"

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy $(WORKSPACE_FLAGS) -- -D warnings

lint-fix:
	$(CARGO) clippy $(WORKSPACE_FLAGS) --fix --allow-dirty --allow-staged -- -D warnings

test:
	$(CARGO) test --workspace --all-features --locked

doc:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps --locked

deny:
	$(CARGO) deny check

qa-local:
	bash scripts/qa/run_local_qa.sh

clean:
	$(CARGO) clean
