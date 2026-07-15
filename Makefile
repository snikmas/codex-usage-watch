PREFIX ?= $(HOME)/.local

.PHONY: build test lint install verify upgrade uninstall smoke package release

build:
	cargo build --release --locked

test:
	cargo test --locked --all-targets

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings

install:
	PREFIX="$(PREFIX)" scripts/install.sh

verify:
	PREFIX="$(PREFIX)" scripts/verify-install.sh

upgrade: install verify

uninstall:
	PREFIX="$(PREFIX)" scripts/uninstall.sh --confirm

smoke:
	bash scripts/smoke-install.sh

package:
	cargo package --locked --allow-dirty

release:
	bash scripts/package-release.sh
