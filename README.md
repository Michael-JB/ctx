# ctx

The way I write code has changed. With agents, I context-switch more, and the
time it takes to spin up or tear down a context became increasingly noticeable.
I didn't enjoy the `git worktree` UX, so I ended up maintaining multiple
long-lived clones of the same repository nestled in different tmux sessions to
work concurrently. This worked well for me, but it wasn't exactly elegant. So
here we are.

`ctx` lets you manage repo-scoped work contexts: each context is a fresh full
checkout on its own local branch, wrapped in a terminal multiplexer session
with your panes already laid out.


Disclaimer: this is fully vibe-coded.

## Install

```sh
uv tool install git+https://github.com/Michael-JB/ctx
```

Or from a local checkout, e.g. for development:

```sh
uv tool install --editable .
```

## Usage

```sh
# One-time per repository: keep a local bare mirror to clone from.
ctx repo add https://github.com/Michael-JB/papaya-nvim.git
ctx repo list
ctx repo rm papaya-nvim            # removes the mirror only

# Create a context: a fresh clone on a new branch off the up-to-date
# default branch, in its own session. Names are globally unique.
ctx new papaya-nvim my-cool-feature
ctx new papaya-nvim follow-up -b other-base   # base on another branch

ctx list                           # branch, dirty/unpushed flags, session state
ctx open my-cool-feature           # attach, recreating the session if needed
ctx rm my-cool-feature             # refuses if dirty/unpushed; --force overrides
```

## Configuration

Optional, at `$XDG_CONFIG_HOME/ctx/config.toml` (`~/.config/ctx/config.toml`).
All fields shown with their defaults, except the layout: that defaults to a
single shell pane, so a custom tree is shown instead.

```toml
contexts_dir = "~/.local/share/ctx/contexts"  # where checkouts live
repos_dir = "~/.local/share/ctx/repos"        # where the bare mirrors live
branch_prefix = ""                            # work branch prefix, e.g. "jane/"
multiplexer = "tmux"                          # or "zellij"

# The pane layout: a tree of panes and "row"/"column" splits ("row" = side
# by side, "column" = stacked). A pane runs `command` (default: a shell) in
# the checkout; at most one pane may set `focus`.
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

## TODOs

- [ ] Add a TUI or interactive mode (?)
- [ ] Add a default project
- [ ] Project-specific layouts (e.g. for `uv run nvim`)
- [ ] Release/versioning mechanism
