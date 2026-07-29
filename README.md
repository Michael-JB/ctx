# ctx

Manage repo-scoped work contexts: each context is a fresh full checkout on
its own local branch, wrapped in a terminal multiplexer session with your
panes already laid out.

## Install

```sh
uv tool install --editable .
```

## Usage

```sh
# One-time per repository: keep a local bare mirror to clone from.
ctx repo add https://github.com/Michael-JB/papaya-nvim.git
ctx repo list
ctx repo rm papaya-nvim            # removes the mirror only

# Create a context: fetches the mirror, clones it, points origin at the
# real URL, branches off the fresh default branch (not pushed), then
# builds and attaches the session.
ctx new papaya-nvim my-cool-feature

# Base a context on a non-default branch (fetched fresh from origin).
ctx new papaya-nvim follow-up -b other-base

# Manage contexts. Names are globally unique (enforced by ctx new), so
# contexts are referenced by name alone; the session is named repo--name.
ctx list                           # branch, dirty/unpushed flags, session state
ctx open my-cool-feature           # attach, recreating the session if needed
ctx rm my-cool-feature             # refuses if dirty/unpushed; --force overrides
```

## Configuration

Optional, at `$XDG_CONFIG_HOME/ctx/config.toml` (`~/.config/ctx/config.toml`):

- `contexts_dir`: where checkouts live
  (default `$XDG_DATA_HOME/ctx/contexts`, i.e. `~/.local/share/ctx/contexts`)
- `repos_dir`: where the internal bare mirrors live
  (default `$XDG_DATA_HOME/ctx/repos`)
- `branch_prefix`: prepended to the work branch name, e.g. `"jane/"`
  (default: none, the branch is named after the context)
- `multiplexer`: `tmux` (default) or `zellij`
- `[layout]`: the pane layout, see below

## Layout

The pane layout is a tree of panes and `row`/`column` splits (`row` places
panes side by side, `column` stacks them). A pane runs `command`, or a plain
shell if omitted; at most one pane may set `focus`. Every pane starts in the
context's checkout. The default is a single shell pane; a lazygit/editor/agent
setup looks like:

```toml
[layout]
split = "row"

[[layout.panes]]
split = "column"
[[layout.panes.panes]]
command = "lazygit"
[[layout.panes.panes]]
command = "nvim"

[[layout.panes]]
command = "claude"
focus = true
```
