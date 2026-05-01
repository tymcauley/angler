.DEFAULT_GOAL := help
.PHONY: help check test test-rust test-fish fmt-check lint build install

help: ## Show this help
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

check: fmt-check lint test ## Run fmt-check + lint + test (full pre-push verification)

test: test-rust test-fish ## Run all tests (Rust + fish)

test-rust: ## Run cargo test
	cargo test

test-fish: ## Run fish render tests
	fish -N tests/fish_render.fish

fmt-check: ## Verify rustfmt-clean (no rewrites)
	cargo fmt --all --check

lint: ## Run clippy with warnings as errors
	cargo clippy --all-targets -- -D warnings

build: ## Build the daemon in release mode
	cargo build --release

install: build ## Install the daemon to ~/.cargo/bin
	cargo install --path .
