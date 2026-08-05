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
- Key semantics should be consistent across panels. Never reuse a key for a different
  action elsewhere.
- Keys that do the same thing are written `a / b` in help text.
