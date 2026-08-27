# ctx

The way I write code has changed. With agents, I context-switch more, and the
time it takes to spin up or tear down a context became increasingly noticeable.
I didn't enjoy the `git worktree` UX, so I ended up maintaining multiple
long-lived clones of the same repository nestled in different tmux sessions to
work in parallel. This worked well for me, but it wasn't exactly elegant. So
here we are.

`ctx` lets you manage repo-scoped work contexts: each context is a fresh full
checkout on its own local branch, wrapped in a terminal multiplexer session
with your panes already laid out.


Disclaimer: this is fully vibe-coded.

## Install

```sh
uv tool install ctx-tui
```

## Setup

`ctx` runs fine with zero config; setup is optional but will likely improve
your experience. The fastest way through it is to not do it yourself: tell your
agent "set up ctx following its README" and let it work through this section.

Instructions for the agent: ask the user each question, then apply their
answer using the concrete snippets under Configuration.

1. Which multiplexer do you use, tmux or zellij? Set `multiplexer` in
   the config file.
2. Which panes should every context session open with (agent, editor,
   lazygit, ...), and which gets focus? Write the `layout` table.
3. Which additional status columns do you want? For the `github` builtin, check
   `gh auth status` and whether the terminal uses a nerd font; for the `agent`
   builtin, install the hooks from the Agent builtin section.
4. Which key should summon the picker? Bind it in the multiplexer's config
   as shown under Instant picker.
5. Should the agent be able to use ctx itself? Install the docs as shown
   under Agent docs.
6. Verify: register a repo (`ctx repo add`), create a context (`ctx new`),
   and check that `ctx list` shows the configured columns.

## Usage

Run `ctx` to manage contexts and repos interactively in the TUI (`?` lists the
keybindings). The TUI is meant to be summoned from inside a multiplexer
session: hit the picker key (see Instant picker) and it opens as a floating
pane over the session you're in.

You can also use `ctx` as a CLI:

```sh
ctx repo add https://github.com/Michael-JB/papaya-nvim.git   # once per repo
ctx new papaya-nvim my-cool-feature   # fresh checkout + session, jump in
# ...work, commit, push...
ctx archive my-cool-feature           # set it aside for later...
ctx rm my-cool-feature                # ...or tear it down
```

More detail:

```sh
# List contexts with their repo, branch, and status:
ctx list

# Contexts branch off the up-to-date default branch. To base one on another branch:
ctx new papaya-nvim follow-up -b other-base

# Leave the name out to get a generated one (e.g. holy-tiger); the TUI's new-context
# prompt pre-fills the same, so enter accepts it and typing replaces it:
ctx new papaya-nvim

# Re-attach to a context, unarchiving it and recreating its session if needed:
ctx open my-cool-feature

# Show, set, or clear the default repo for new contexts:
ctx repo default
ctx repo default papaya-nvim
ctx repo default --clear

# List registered repos, or remove some (their contexts are left alone):
ctx repo list
ctx repo rm papaya-nvim

# Set contexts aside without deleting them (kills their sessions), and bring one back.
# Archived contexts keep their names: names are unique across live and archived.
ctx archive my-cool-feature
ctx unarchive my-cool-feature

# List archived contexts, or empty the whole archive:
ctx list --archived
ctx archive --empty
```

## Configuration

Configure `ctx` via `$XDG_CONFIG_HOME/ctx/config.toml`. All fields shown with
their defaults.

```toml
contexts_dir = "~/.local/share/ctx/contexts"  # where checkouts live
repos_dir = "~/.local/share/ctx/repos"        # internal storage for registered repos
archive_dir = "~/.local/share/ctx/archive"    # where archived contexts go
branch_prefix = ""                            # work branch prefix, e.g. "jane/"
multiplexer = "tmux"                          # or "zellij" (requires zellij >= 0.44)
nerd_font = true                              # false swaps builtin glyphs for plain unicode
```

### Multiplexer layout

`ctx` supports `tmux` and `zellij` multiplexers. Customise the layout of a
`ctx` session via the `layout` table in the config:

```toml
# The pane layout: a tree of panes and "row"/"column" splits ("row" = side
# by side, "column" = stacked). A pane runs `command` or a `builtin`
# (default: a shell) in the checkout; at most one pane may set `focus`.
[layout]
split = "row"

[[layout.panes]]
split = "column"
[[layout.panes.panes]]
command = "lazygit"
[[layout.panes.panes]]
command = "nvim"

[[layout.panes]]
builtin = "claude"
focus = true
```

#### Pane builtins

A pane can use a `builtin` instead of a command. Where a `command` pane
always runs the same string, a builtin names something `ctx` knows how to
run, so it can compose the invocation itself. Extra flags go in `args`; a
command to run the invocation through (an environment loader, a package
manager's `run`, ...) goes in `wrap`.

The `claude` builtin runs Claude Code:

```toml
[[layout.panes]]
builtin = "claude"
args = "--model opus"      # optional extra flags
wrap = "direnv exec ."     # optional: runs `direnv exec . claude --model opus`
```

`ctx new --set prompt="..."` hands it an initial prompt, so an agent (or
you) can spin off an exploration into its own context:

```sh
ctx new myrepo --set prompt="explore the flaky login test"
```

Recreating the session later (`ctx open` after a kill or an unarchive)
resumes the checkout's conversation via `claude --continue`.

#### Instant picker

In your multiplexer's config, bind a key that opens the TUI as a floating
overlay in whatever session you're in. zellij (`config.kdl`):

```kdl
bind "Alt c" {
    Run "ctx" "tui" "--exit" {
        floating true
        close_on_exit true
    }
}
```

tmux: `bind -n M-c display-popup -E "ctx tui --exit"`.

### Theme

The TUI renders with your terminal's ANSI palette. A `[theme]` table overrides
individual colours with hex values, e.g. to match a truecolor lazygit theme:

```toml
[theme]
foreground = "#c8d3f5"
selection = "#2d3f76"       # the focused panel's cursor row
border_active = "#ff966c"
border_inactive = "#589ed7"
```

### Status columns

The STATUS column shows the git state: `*` for uncommitted changes, `↑n` for n
unpushed commits.

You can add further columns via `[[status]]`. `ctx` comes with some builtin
status integrations. The TUI re-polls every few seconds and colours known
states.

#### GitHub builtin

```toml
[[status]]
name = "pr"
builtin = "github"         # the branch's latest PR, collapsed into one cell
```

Requires an authenticated `gh` in the checkout (without one the cells stay
blank), and a [nerd font](https://www.nerdfonts.com): the states render as
nerd-font glyphs (set `nerd_font = false` to fall back to plain Unicode).

#### Agent builtin

```toml
[[status]]
name = "agent"
builtin = "agent"          # .git/agent-status, hook-written by your agent
```

You'll also need to configure your agent to write its status, e.g., for Claude
Code, add the following to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "ctx builtin claude status-hook"}]}],
    "PreToolUse": [{"hooks": [{"type": "command", "command": "ctx builtin claude status-hook"}]}],
    "Notification": [{"matcher": "permission_prompt|elicitation_dialog|agent_needs_input", "hooks": [{"type": "command", "command": "ctx builtin claude status-hook"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "ctx builtin claude status-hook"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "ctx builtin claude status-hook"}]}]
  }
}
```

#### Custom

You can also add your own status column via a command that runs in the
checkout:

```toml
[[status]]
name = "anything"
command = "my-status"      # first line of any command, run in the checkout
```

### Agent docs

To make your agent ctx-aware, install the output of `ctx agent-docs` as
a skill. E.g., for Claude Code:

```sh
mkdir -p ~/.claude/skills/ctx && ctx agent-docs > ~/.claude/skills/ctx/SKILL.md
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

Shell panes pick this up through direnv's shell hook. Command and builtin
panes don't: the multiplexer starts them directly, not from your shell. Give
a builtin pane the env via `wrap = "direnv exec ."`, and a command pane by
writing it out (`command = "direnv exec . lazygit"`).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
