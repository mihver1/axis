default:
    @just --list

doctor:
    @bash scripts/bootstrap-macos.sh --doctor

check:
    cargo check --workspace

run:
    cargo run -p canvas-app

fmt:
    cargo fmt --all

