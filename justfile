default:
    @just --list

doctor:
    @bash scripts/bootstrap-macos.sh --doctor

check:
    cargo check --workspace

run:
    cargo run -p axis-app

smoke-acp:
    bash scripts/smoke-acp-demo.sh

fmt:
    cargo fmt --all
