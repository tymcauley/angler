.PHONY: test test-rust test-fish build install

test: test-rust test-fish

test-rust:
	cargo test

test-fish:
	fish -N tests/fish_render.fish

build:
	cargo build --release

install: build
	cargo install --path .
