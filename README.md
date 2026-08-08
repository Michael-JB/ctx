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
ctx archive my-cool-feature           # or set it aside for later...
ctx rm my-cool-feature                # ...or tear it all down again
```

Not ready to tear a context down? Archive it instead (in the TUI, `d` offers
Archive next to Delete): its session is killed and the checkout moves aside to
`archive_dir`, freeing the name. The TUI's archived panel lets you unarchive
(`u`, or enter to also open it), delete permanently (`d`), or empty the whole
archive (`e`).

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

# Set contexts aside without deleting them (kills their sessions), and bring one back:
ctx archive my-cool-feature
ctx unarchive my-cool-feature

# List archived contexts, delete some permanently, or empty the whole archive:
ctx list --archived
ctx rm --archived my-cool-feature
ctx archive --empty
```

## Configuration

Optional, at `$XDG_CONFIG_HOME/ctx/config.toml`. Directories default under
`$XDG_DATA_HOME/ctx/`; the example shows the usual fallback paths. All fields
shown with their defaults, except the layout: that defaults to a single shell
pane, so a custom tree is shown instead.

```toml
contexts_dir = "~/.local/share/ctx/contexts"  # where checkouts live
repos_dir = "~/.local/share/ctx/repos"        # internal storage for registered repos
archive_dir = "~/.local/share/ctx/archive"    # where archived contexts go
branch_prefix = ""                            # work branch prefix, e.g. "jane/"
multiplexer = "tmux"                          # or "zellij" (requires zellij >= 0.44)
# [[status]] tables add extra columns; see "Status columns" (none by default)

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

### Status columns

The STATUS column shows the git state: `*` for uncommitted changes, `↑n`
for n unpushed commits. `[[status]]` tables add further columns:

```toml
[[status]]
name = "pr"
builtin = "github-pr"      # the branch's latest PR: open / draft / merged / closed

[[status]]
name = "ci"
builtin = "github-checks"  # checks on the branch's open PR: success / failure / pending

[[status]]
name = "claude"
builtin = "agent"          # .git/agent-status, hook-written by your agent (see below)

[[status]]
name = "anything"
command = "my-status"      # first line of any command, run in the checkout
```

An empty cell means "no status": no PR, a stale agent file, a silent or
failing command. The GitHub built-ins need an authenticated `gh`. The TUI
re-polls every few seconds and colours known states.

For the `agent` column, make your agent write its state on its lifecycle hooks
to `.git/agent-status`. For example, for Claude Code, add to
`~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "[ -d .git ] && echo working > .git/agent-status || true"}]}],
    "PreToolUse": [{"hooks": [{"type": "command", "command": "[ -d .git ] && echo working > .git/agent-status || true"}]}],
    "Notification": [{"matcher": "permission_prompt|elicitation_dialog|agent_needs_input", "hooks": [{"type": "command", "command": "[ -d .git ] && echo blocked > .git/agent-status || true"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "[ -d .git ] && echo idle > .git/agent-status || true"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "rm -f .git/agent-status"}]}]
  }
}
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
