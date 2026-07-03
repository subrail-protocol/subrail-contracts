.PHONY: test lint fmt build clean

test:
	cargo test --workspace

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --all

build:
	cargo build --target wasm32v1-none --release

clean:
	cargo clean
