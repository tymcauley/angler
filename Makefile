.PHONY: check test test-rust test-fish fmt-check lint build install

check: fmt-check lint test

test: test-rust test-fish

test-rust:
	cargo test

test-fish:
	fish -N tests/fish_render.fish

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --all-targets -- -D warnings

build:
	cargo build --release

install: build
	cargo install --path .
