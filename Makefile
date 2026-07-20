.PHONY: check fmt host-test compositor-test ui-install ui-test ui-build firmware dist

check:
	nix develop --command ./scripts/check.sh

fmt:
	nix develop --command cargo fmt --all
	nix develop --command cargo fmt --manifest-path crates/glove80-host-protocol/Cargo.toml
	nix develop --command cargo fmt --manifest-path crates/glove80-compositor/Cargo.toml
	nix develop --command cargo fmt --manifest-path firmware/glove80-rmk/Cargo.toml

host-test:
	nix develop --command cargo test --workspace
	nix develop --command cargo test --manifest-path crates/glove80-host-protocol/Cargo.toml

compositor-test:
	nix develop --command cargo test --manifest-path crates/glove80-compositor/Cargo.toml

ui-install:
	npm ci --prefix ui

ui-test: ui-install
	npm test --prefix ui

ui-build: ui-install
	npm run build --prefix ui

firmware dist:
	nix develop --command ./scripts/build-release.sh
