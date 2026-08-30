#!/usr/bin/env sh
# Source of truth for the repo's checks; CI runs this script too.
# --fix applies formatting and lint fixes instead of just checking.
set -e

if [ "$1" = "--fix" ]; then
    cargo fmt
    cargo clippy --fix --allow-dirty --allow-staged --all-targets -- -D warnings
else
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
fi
cargo test
