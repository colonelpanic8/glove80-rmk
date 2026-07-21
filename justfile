default:
  @just --list

fmt:
  cargo fmt --all
  cargo fmt --manifest-path crates/glove80-rmk/Cargo.toml

check:
  cargo run --quiet -p xtask -- check

host-test:
  cargo test --workspace

firmware: dist

dist:
  cargo run --quiet -p xtask -- dist

inspect-uf2 file:
  cargo run --quiet -p xtask -- inspect-uf2 "{{file}}"
