---
name: ctx
description: Manage isolated work contexts (fresh repo checkout + multiplexer session) with the ctx CLI. Use when asked to interact with ctx.
---

# Working with ctx contexts

A context is a fresh clone of a registered repo (`ctx repo list`) on its
own branch, wrapped in a terminal multiplexer session. Spin one off in
the background:

    ctx new <repo> <name> --detach

- Without `--detach` the command switches the user's terminal to the new
  session; keep it unless the user asks to jump straight in.
- Report the name so the user can jump in with `ctx open <name>`.
- `-b <branch>` bases the context on a branch other than the default.

Given a task, seed it into the context:

    ctx new <repo> <name> --detach --set prompt="investigate the flaky login test"

The seeded prompt is answered by the agent inside the new context, not by
you. After creating the context, report its name and stop: do not work on
the task yourself.

If `--set` is rejected, let the user know that seeding requires a builtin pane
in their ctx layout, and create the context unseeded.

Clean up after yourself: archive contexts with `ctx archive <name>` when you're
done. Permanently delete a context with `ctx rm <name>`; do this only when
explicitly instructed. `ctx --help` covers the rest.

## Upgrading ctx

1. Capture the installed version: `ctx --version`.
2. Upgrade the binary (e.g. `cargo install ctx-tui`).
3. Run `ctx changelog` and apply the Upgrade Notes sections between the
   old and the new version.
4. Refresh this skill: `ctx agent-docs > <this skill's file>`.

The user's ctx config lives at `$XDG_CONFIG_HOME/ctx/config.toml`.
