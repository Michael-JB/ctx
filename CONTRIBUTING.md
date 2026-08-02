# Contributing

## Development setup

Run from a checkout with [uv](https://docs.astral.sh/uv/):

```sh
uv run ctx
```

Or install your checkout as the `ctx` command:

```sh
uv tool install --editable .
```

## Checks

```sh
uv run ruff format --check .
uv run ruff check .
uv run mypy src
```

## Commit messages

- Use [conventional commit](https://www.conventionalcommits.org) prefixes
  (`feat:`, `fix:`, `docs:`, ...); releases derive versions and the
  changelog from them.
- Keep each commit atomic: one self-contained change per commit.
- Explain the why in the body when it isn't obvious; the diff already
  shows the what.

## Releases

Releases are cut by [release-please](https://github.com/googleapis/release-please):
merging its release PR bumps the version, updates the changelog, and tags a
GitHub release, which publishes to PyPI.
