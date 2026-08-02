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
uv tool install ctx-tui
```

## Usage

Run `ctx` to manage contexts and repos interactively in the TUI (`?` lists
the keybindings). The same operations are available as subcommands:

```sh
ctx repo add https://github.com/Michael-JB/papaya-nvim.git   # once per repo
ctx new papaya-nvim my-cool-feature   # fresh checkout + session, jump in
# ...work, commit, push...
ctx rm my-cool-feature                # tear it all down again
```

More detail:

```sh
# List contexts with their repo, branch, and status:
ctx list

# Contexts branch off the up-to-date default branch. To base one on another branch:
ctx new papaya-nvim follow-up -b other-base

# Re-attach to a context, recreating its session if needed:
ctx open my-cool-feature

# List registered repos, or remove some (their contexts are left alone):
ctx repo list
ctx repo rm papaya-nvim
```

## Configuration

Optional, at `$XDG_CONFIG_HOME/ctx/config.toml`. Directories default under
`$XDG_DATA_HOME/ctx/`; the example shows the usual fallback paths. All fields
shown with their defaults, except the layout: that defaults to a single shell
pane, so a custom tree is shown instead.

```toml
contexts_dir = "~/.local/share/ctx/contexts"  # where checkouts live
repos_dir = "~/.local/share/ctx/repos"        # internal storage for registered repos
branch_prefix = ""                            # work branch prefix, e.g. "jane/"
multiplexer = "tmux"                          # or "zellij" (requires zellij >= 0.44)

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

### Environment variables

Fresh checkouts don't carry untracked files like `.env`. All contexts of a
repo share the parent directory `<contexts_dir>/<repo>/`; you could, for
example, use a tool like [direnv](https://direnv.net) to export env vars in
all of a repo's contexts from a single `.envrc` there:

```sh
echo 'export MY_SECRET=some-value' > <contexts_dir>/papaya-nvim/.envrc
direnv allow <contexts_dir>/papaya-nvim
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
