# Architecture

- Stay abstract over integration points: the forge (GitHub, GitLab, ...), the
terminal multiplexer (tmux, zellij), and the agent system (Claude, Codex,
OpenCode, ...) feeding statuses. Provider specifics live only behind their
interface (the forge derived from a repo's remote, the config-selected
`multiplexer` backend, and the `.git/agent-status` file contract.

# Commits

- Commits are self-contained, with a single purpose.
- Use conventional commits. The changelog is generated from commit titles:
  `feat`, `fix`, `perf`, `deps`, `revert`, and `docs` become entries, so use
  each once per visible change or the changelog fills with noise.
- Use `imp` for incremental steps on the way to a larger change.

# Checks

Run `./lint.sh` before every commit. It is the source of truth for checks;
CI runs it too.

# TUI

- Prompts: assume the user knows what an action does. Never explain it.
- Keys are panel-scoped, lazygit-style. A key reused across panels must keep the
  same spirit (d removes the panel's kind of thing, n creates one).
- Keys that do the same thing are written `a / b` in help text.
