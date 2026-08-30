# Contributing

## Development setup

Run from a checkout with [cargo](https://doc.rust-lang.org/cargo/):

```sh
cargo run --
```

Or install your checkout as the `ctx` command:

```sh
cargo install --path .
```

## Checks

```sh
./lint.sh        # check; CI runs this on every pull request
./lint.sh --fix  # also apply formatting and lint fixes
```

## Commit messages

- Use [conventional commit](https://www.conventionalcommits.org) prefixes
  (`feat:`, `fix:`, `docs:`, ...); releases derive versions and the
  changelog from them.
- Keep each commit atomic: one self-contained change per commit.
- Explain the why in the body when it isn't obvious; the diff already
  shows the what.
- A change that needs user action after upgrading (a config edit, a
  command to run) declares it as an extra `upgrade: ensure ...` line in
  the commit body. These render as the release's Upgrade Notes section,
  which agents apply when they upgrade ctx (via `ctx changelog`).

## XXX comments

Mark known compromises with an `XXX:` comment saying why the code exists,
together with a removal condition.

## Releases

Releases are cut by [release-please](https://github.com/googleapis/release-please):
merging its release PR bumps the version, updates the changelog, and tags a
GitHub release, which publishes to crates.io and uploads prebuilt binaries.

## Website

The project website is static-served from `docs/`. To prototype locally:

```sh
python3 -m http.server -d docs
```

Then open <http://localhost:8000>.

## TODOs

- [x] Add a TUI or interactive mode
- [x] Add a default project
- [ ] Project-specific layouts (e.g. for `uv run nvim`)
- [x] Release/versioning mechanism
- [x] Support for pulling in environment vars (maybe .env files?)
- [ ] Post-create hook command (e.g. `direnv allow`, `uv sync`)
