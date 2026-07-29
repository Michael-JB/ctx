# ctx

Manage repo-scoped work contexts: each context is a fresh full checkout on its
own local branch, wrapped in a tmux session with the panes already laid out
(lazygit top-left, nvim bottom-left, claude right).

## Install

```sh
uv tool install --editable .
```

## Usage

```sh
# One-time per repository: keep a local bare mirror to clone from.
ctx repo add https://github.com/Michael-JB/papaya-nvim.git
ctx repo list
ctx repo remove papaya-nvim        # removes the mirror only

# Create a context: fetches the mirror, clones it, points origin at the
# real URL, branches mb/<name> off the fresh default branch (not pushed),
# then builds and attaches the tmux session.
ctx new papaya-nvim my-cool-feature

# Base a context on a non-default branch (fetched fresh from origin).
ctx new papaya-nvim follow-up --branch other-base

# Manage contexts. Names are globally unique (enforced by ctx new), so
# contexts are referenced by name alone; the tmux session is repo--name.
ctx list                           # branch, dirty/unpushed flags, session state
ctx open my-cool-feature           # attach, recreating the session if needed
ctx rm my-cool-feature             # refuses if dirty/unpushed; --force overrides
```

## Layout

- Mirrors: `~/.local/share/ctx/repos/<repo>.git`
- Checkouts: `~/dev/contexts/<repo>/<name>`
- Optional config: `~/.config/ctx/config.toml` with `contexts_dir`,
  `repos_dir`, and `branch_prefix` (default `mb/`).
