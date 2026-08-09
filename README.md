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

Fastest path: ask your agent to set `ctx` up ("set up ctx following its
README"). This section is everything it, or you, needs to do. `ctx` works
with zero config; these three steps unlock the rest.

**1. Config**, at `~/.config/ctx/config.toml` (respects `$XDG_CONFIG_HOME`):
pick your multiplexer and the panes every context session opens with.

```toml
multiplexer = "zellij"   # or "tmux" (the default); zellij needs >= 0.44

[layout]
split = "row"

[[layout.panes]]
command = "lazygit"

[[layout.panes]]
command = "claude"
focus = true
```

**2. Agent status hooks.** The AGENT column (on by default) reads
`.git/agent-status` from each checkout. For Claude Code, merge these hooks
into the `hooks` table of `~/.claude/settings.json`, keeping any existing
entries. States are working / monitoring / blocked / idle; the file is
rewritten only when the state changes, so its mtime dates the state and
active states show their age (`working 12m`).

```json
{
  "hooks": {
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "[ -d .git ] || exit 0; [ \"$(cat .git/agent-status 2>/dev/null)\" = working ] || echo working > .git/agent-status"}]}],
    "PreToolUse": [{"hooks": [{"type": "command", "command": "[ -d .git ] || exit 0; state=$(jq -r 'if .tool_name == \"Monitor\" or .tool_name == \"ScheduleWakeup\" then \"monitoring\" else \"working\" end' 2>/dev/null); [ -n \"$state\" ] || state=working; [ \"$(cat .git/agent-status 2>/dev/null)\" = \"$state\" ] || echo \"$state\" > .git/agent-status"}]}],
    "Notification": [{"matcher": "permission_prompt|elicitation_dialog|agent_needs_input", "hooks": [{"type": "command", "command": "[ -d .git ] && echo blocked > .git/agent-status || true"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "[ -d .git ] && echo idle > .git/agent-status || true"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "rm -f .git/agent-status"}]}]
  }
}
```

**3. Instant picker.** Bind a key that opens the TUI as an overlay in
whatever session you're in; picking a context re-points the client, so no
dedicated picker session is needed. zellij (`config.kdl`):

```kdl
bind "Alt c" {
    Run "ctx" "tui" "--exit" {
        floating true
        close_on_exit true
    }
}
```

tmux: `bind -n M-c display-popup -E "ctx tui --exit"`.

**Verify:** `ctx list` shows NAME / REPO / BRANCH / STATUS / AGENT / PR
columns. With a repo added and a Claude session running in a context, AGENT
shows e.g. `working 3m` after a prompt; PR fills in once the branch has a PR
(needs an authenticated `gh`).

## Usage

Run `ctx` to manage contexts and repos interactively in the TUI (`?` lists
the keybindings). You can also use `ctx` as a CLI.

```sh
ctx repo add https://github.com/Michael-JB/papaya-nvim.git   # once per repo
ctx new papaya-nvim my-cool-feature   # fresh checkout + session, jump in
# ...work, commit, push...
ctx archive my-cool-feature           # set it aside for later...
ctx rm my-cool-feature                # ...or tear it down
```

Out of the box, in the TUI:

- Keys are lazygit-style and panel-scoped; `?` shows the current panel's.
- The attached context is pinned on top with the cursor on the row below, so
  enter right after opening switches to your previous session.
- `a` archives instantly (cheap to undo with `u`); `d` deletes with a
  confirmation that warns about uncommitted or unpushed work.
- Deleting or archiving the session you're in switches you to the next one.
- `o` opens the context's PR in the browser.
- `s` on a repo makes it the default for new contexts, wherever `n` is
  pressed; the repos panel itself creates in the hovered repo.
- `/` fuzzy-filters the focused panel by name.

More CLI:

```sh
# List contexts with their repo, branch, and status:
ctx list

# Contexts branch off the up-to-date default branch. To base one on another branch:
ctx new papaya-nvim follow-up -b other-base

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
```

### Multiplexer layout

The `layout` table is a tree of panes and "row"/"column" splits ("row" = side
by side, "column" = stacked). A pane runs `command` (default: a shell) in the
checkout; at most one pane may set `focus`. A nested example:

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

### Theme

The TUI renders with your terminal's ANSI palette. A `[theme]` table overrides
individual colours with hex values or Textual colour names, e.g. to match a
truecolor lazygit theme:

```toml
[theme]
foreground = "#c8d3f5"
selection = "#2d3f76"       # the focused panel's cursor row
border_active = "#ff966c"
border_inactive = "#589ed7"
```

### Status columns

The STATUS column shows the git state: `*` for uncommitted changes, `↑n` for n
unpushed commits. Two more columns are on by default: AGENT (the `agent`
builtin, fed by the hooks in Setup) and PR (the `github` builtin).

Configuring any `[[status]]` replaces the defaults entirely, so re-declare
the columns you want to keep.

#### GitHub builtin

Requires an authenticated `gh` in the checkout (without one the cells stay
blank). The `github` builtin collapses the branch's latest PR into one cell,
showing its most urgent fact:

| state | meaning | shown as |
|---|---|---|
| `merged` | PR merged | `◆` magenta |
| `closed` | PR closed unmerged | `⊘` red |
| `conflicts` | merge conflicts | `⚠` yellow |
| `failing` | CI failure on the head commit | `✖` red |
| `draft` | draft PR | `✎` dim |
| `pending` | CI still running | `◌` yellow |
| `ready` | open, mergeable, CI green or absent | `✔` green |

The narrower `github-pr` (open / draft / merged / closed) and `github-checks`
(success / failure / pending) builtins remain for a two-column split.

#### Icons and colours

Any column's icons and colours are configurable (e.g. for nerd fonts and
truecolor themes). Both key on a cell's leading word, so timed cells like
`working 12m` are covered too:

```toml
[[status]]
name = "pr"
builtin = "github"
[status.icons]
merged = "M"
[status.styles]
merged = "bold #c099ff"
```

#### Custom

You can also add your own status column via a command that runs in the
checkout. Agents other than Claude Code can feed the `agent` builtin by
writing their state word to `.git/agent-status` as described in Setup.

```toml
[[status]]
name = "anything"
command = "my-status"      # first line of any command, run in the checkout
```

The TUI re-polls every few seconds and colours known states.

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
