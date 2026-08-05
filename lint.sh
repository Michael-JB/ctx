#!/usr/bin/env sh
# Source of truth for the repo's checks; CI runs this script too.
# --fix applies formatting and lint fixes instead of just checking.
set -e

if [ "$1" = "--fix" ]; then
    uv run ruff format .
    uv run ruff check --fix .
else
    uv run ruff format --check .
    uv run ruff check .
fi
uv run mypy src
uv run pytest
