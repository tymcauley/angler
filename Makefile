.DEFAULT_GOAL := help
.PHONY: help check test test-rust test-fish fmt-check lint build install install-fish uninstall

FISH_CONFIG_DIR ?= $(or $(XDG_CONFIG_HOME),$(HOME)/.config)/fish

help: ## Show this help
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

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

install: install-fish ## Install daemon (~/.cargo/bin) and symlink fish files into FISH_CONFIG_DIR
	cargo install --path .

install-fish: ## Symlink every *.fish in conf.d/ and functions/ into FISH_CONFIG_DIR
	@mkdir -p $(FISH_CONFIG_DIR)/conf.d $(FISH_CONFIG_DIR)/functions
	@for f in conf.d/*.fish functions/*.fish; do \
	    target="$(FISH_CONFIG_DIR)/$$f"; \
	    ln -sf "$(CURDIR)/$$f" "$$target"; \
	    echo "  $$f -> $$target"; \
	done

uninstall: ## Remove our symlinks from FISH_CONFIG_DIR (binary stays under cargo's management)
	@for f in conf.d/*.fish functions/*.fish; do \
	    target="$(FISH_CONFIG_DIR)/$$f"; \
	    if [ -L "$$target" ] && [ "$$(readlink "$$target")" = "$(CURDIR)/$$f" ]; then \
	        rm -f "$$target"; \
	        echo "  removed $$target"; \
	    fi; \
	done
