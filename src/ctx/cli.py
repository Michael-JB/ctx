import asyncio
import importlib.resources
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import click

from ctx import claude_hook, contexts, repos, status
from ctx.config import Config, ConfigError, load_config
from ctx.layout import LayoutError, accepted_keys
from ctx.multiplexer import Multiplexer, MultiplexerError, get_multiplexer


@dataclass(frozen=True)
class Deps:
    """Injectable dependencies shared by all commands."""

    cfg: Config
    mux: Multiplexer


@click.group(invoke_without_command=True)
@click.version_option(package_name="ctx-tui")
@click.pass_context
def cli(click_ctx: click.Context) -> None:
    """Manage repo-scoped work contexts."""
    if click_ctx.obj is None:
        cfg = load_config()
        click_ctx.obj = Deps(cfg, get_multiplexer(cfg.multiplexer, cfg.layout))
    if click_ctx.invoked_subcommand is None:
        click_ctx.invoke(tui)


@cli.command()
@click.argument("repo")
@click.argument("name", required=False)
@click.option("-b", "--branch", "base", help="Base branch (default: the repo's default branch).")
@click.option(
    "-s",
    "--set",
    "assignments",
    multiple=True,
    metavar="KEY=VALUE",
    help="Pass a value to the layout's builtin panes (e.g. prompt=... for claude).",
)
@click.option("-d", "--detach", is_flag=True, help="Start the session without attaching to it.")
@click.pass_obj
def new(
    deps: Deps,
    repo: str,
    name: str | None,
    base: str | None,
    assignments: tuple[str, ...],
    detach: bool,
) -> None:
    """Create a context: fresh checkout of REPO on a new local branch.

    NAME defaults to a random adjective-animal pair.
    """
    values = _parse_assignments(deps.cfg, assignments)
    _create_and_open(deps, repo, name, base, values, detach=detach)


def _parse_assignments(cfg: Config, assignments: tuple[str, ...]) -> dict[str, str]:
    accepted = accepted_keys(cfg.layout)
    values: dict[str, str] = {}
    for assignment in assignments:
        key, sep, value = assignment.partition("=")
        if not sep or not key:
            raise click.ClickException(f"--set needs KEY=VALUE, got '{assignment}'")
        if key in values:
            raise click.ClickException(f"--set gives '{key}' twice")
        if key not in accepted:
            raise click.ClickException(f"no builtin pane in the layout accepts '{key}'")
        values[key] = value
    return values


def _create_and_open(
    deps: Deps,
    repo: str,
    name: str | None,
    base: str | None,
    values: dict[str, str],
    detach: bool = False,
) -> None:
    try:
        if name is None:
            name = contexts.random_name(deps.cfg)
        ctx = asyncio.run(contexts.create_context(deps.cfg, repo, name, base))
    except (ValueError, FileExistsError, FileNotFoundError) as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"created {ctx.qualified} at {ctx.path} on {contexts.current_branch(ctx)}")
    try:
        if detach:
            deps.mux.create(ctx, values)
        else:
            deps.mux.open(ctx, values)
    except MultiplexerError as exc:
        raise click.ClickException(str(exc)) from exc


@cli.command("open")
@click.argument("name")
@click.pass_obj
def open_(deps: Deps, name: str) -> None:
    """Attach to a context's session, unarchiving it and recreating the session if needed."""
    try:
        ctx = contexts.find_any(deps.cfg, name)
        if contexts.is_archived(deps.cfg, ctx):
            ctx = contexts.unarchive_context(deps.cfg, ctx)
            click.echo(f"unarchived {ctx.qualified}")
    except (LookupError, FileExistsError) as exc:
        raise click.ClickException(str(exc)) from exc
    try:
        deps.mux.open(ctx)
    except MultiplexerError as exc:
        raise click.ClickException(str(exc)) from exc


async def _all_status_cells(cfg: Config, ctxs: list[contexts.Context]) -> list[list[str]]:
    """Every context's status cells, fetched concurrently."""
    return await asyncio.gather(*(status.status_cells(cfg, ctx) for ctx in ctxs))


@cli.command("list")
@click.option("--archived", is_flag=True, help="List archived contexts instead.")
@click.pass_obj
def list_(deps: Deps, archived: bool) -> None:
    """List contexts with branch, dirtiness, and session state."""
    if archived:
        all_contexts = contexts.list_archived(deps.cfg)
    else:
        all_contexts = contexts.list_contexts(deps.cfg)
    if not all_contexts:
        click.echo("no archived contexts" if archived else "no contexts")
        return
    status_names = tuple(s.name.upper() for s in deps.cfg.status)
    rows = [("NAME", "REPO", "BRANCH", "STATUS", *status_names)]
    cell_rows = asyncio.run(_all_status_cells(deps.cfg, all_contexts))
    for ctx, ctx_cells in zip(all_contexts, cell_rows, strict=True):
        rows.append((ctx.name, ctx.repo, contexts.current_branch(ctx), *ctx_cells))
    widths = [max(len(row[column]) for row in rows) for column in range(len(rows[0]))]
    for row in rows:
        cells = zip(row, widths, strict=True)
        click.echo("  ".join(cell.ljust(width) for cell, width in cells).rstrip())


@cli.command()
@click.argument("names", nargs=-1, required=True)
@click.option("--force", is_flag=True, help="Delete even with uncommitted or unpushed work.")
@click.pass_obj
def rm(deps: Deps, names: tuple[str, ...], force: bool) -> None:
    """Delete contexts, archived or not: kill their sessions and remove the checkouts."""
    failed = False
    for name in names:
        error = _remove_one(deps, name, force)
        if error:
            click.echo(f"error: {error}", err=True)
            failed = True
    if failed:
        sys.exit(1)


def _remove_one(deps: Deps, name: str, force: bool) -> str | None:
    """Delete one context, returning an error message instead of raising."""
    try:
        ctx = contexts.find_any(deps.cfg, name)
    except LookupError as exc:
        return str(exc)
    if not force:
        problems = []
        if contexts.is_dirty(ctx):
            problems.append("uncommitted changes")
        unpushed = contexts.unpushed_commits(ctx)
        if unpushed:
            problems.append(f"{len(unpushed)} unpushed commit(s)")
        if problems:
            return f"{ctx.qualified} has {' and '.join(problems)}; use --force to delete anyway"
    # Kill last: killing our own session takes this process down with it,
    # so nothing after the kill is guaranteed to run. An interrupted
    # removal is finished by the startup sweep.
    contexts.remove_context(ctx)
    click.echo(f"removed {ctx.qualified}")
    if deps.mux.exists(ctx):
        deps.mux.kill(ctx)
    return None


@cli.command()
@click.argument("names", nargs=-1)
@click.option("--empty", is_flag=True, help="Permanently delete all archived contexts.")
@click.pass_obj
def archive(deps: Deps, names: tuple[str, ...], empty: bool) -> None:
    """Archive contexts: kill their sessions and move the checkouts aside."""
    if empty:
        if names:
            raise click.UsageError("--empty takes no context names")
        count = len(contexts.list_archived(deps.cfg))
        contexts.empty_archive(deps.cfg)
        click.echo(f"emptied archive ({count} context(s))")
        return
    if not names:
        raise click.UsageError("provide context names or --empty")
    failed = False
    for name in names:
        error = _archive_one(deps, name)
        if error:
            click.echo(f"error: {error}", err=True)
            failed = True
    if failed:
        sys.exit(1)


def _archive_one(deps: Deps, name: str) -> str | None:
    """Archive one context, returning an error message instead of raising."""
    try:
        ctx = contexts.find_context(deps.cfg, name)
        contexts.archive_context(deps.cfg, ctx)
    except (LookupError, FileExistsError) as exc:
        return str(exc)
    if deps.mux.exists(ctx):
        deps.mux.kill(ctx)
    click.echo(f"archived {ctx.qualified}")
    return None


@cli.command()
@click.argument("name")
@click.pass_obj
def unarchive(deps: Deps, name: str) -> None:
    """Restore an archived context."""
    try:
        archived = contexts.find_archived(deps.cfg, name)
        ctx = contexts.unarchive_context(deps.cfg, archived)
    except (LookupError, FileExistsError) as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"unarchived {ctx.qualified}")


@cli.command()
@click.option("--exit", "exit_on_open", is_flag=True, help="Exit the TUI after opening a context.")
@click.pass_obj
def tui(deps: Deps, exit_on_open: bool) -> None:
    """Manage contexts and repos interactively."""
    from ctx.tui import CtxTui, NewRequest, OpenRequest

    # When the multiplexer can open sessions in place (e.g. inside tmux),
    # the TUI handles everything itself and exits with no request. The
    # requests below are the fallback for terminal-takeover attaches.
    match CtxTui(deps.cfg, deps.mux, exit_on_open=exit_on_open).run():
        case OpenRequest(name=name):
            try:
                ctx = contexts.find_context(deps.cfg, name)
                deps.mux.open(ctx)
            except (LookupError, MultiplexerError) as exc:
                raise click.ClickException(str(exc)) from exc
        case NewRequest(repo=repo_name, name=name, base=base):
            _create_and_open(deps, repo_name, name, base, {})
        case None:
            pass


@cli.command("agent-docs")
def agent_docs() -> None:
    """Print ctx usage docs for coding agents, ready to install as a skill."""
    docs = importlib.resources.files("ctx").joinpath("agent_docs.md").read_text()
    click.echo(docs, nl=False)


@cli.command()
def changelog() -> None:
    """Print the installed version's changelog."""
    text = importlib.resources.files("ctx").joinpath("CHANGELOG.md").read_text()
    click.echo(text, nl=False)


@cli.command("claude-hook")
def claude_hook_() -> None:
    """Feed the agent status column from a Claude Code hook event on stdin."""
    claude_hook.handle(sys.stdin.read(), Path.cwd())


@cli.group()
def repo() -> None:
    """Manage registered repositories."""


@repo.command("add")
@click.argument("url")
@click.option("--name", help="Registry name (default: derived from URL).")
@click.pass_obj
def repo_add(deps: Deps, url: str, name: str | None) -> None:
    """Register a repository by cloning a local bare mirror of it."""
    click.echo(f"cloning {url}")
    try:
        registered = asyncio.run(repos.add_repo(deps.cfg, url, name))
    except FileExistsError as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"registered '{registered}'")


@repo.command("list")
@click.pass_obj
def repo_list(deps: Deps) -> None:
    """List registered repositories."""
    for name in repos.repo_names(deps.cfg):
        click.echo(f"{name}\t{repos.repo_url(deps.cfg, name)}")


@repo.command("default")
@click.argument("name", required=False)
@click.option("--clear", is_flag=True, help="Clear the default repo.")
@click.pass_obj
def repo_default(deps: Deps, name: str | None, clear: bool) -> None:
    """Show or set the repo new contexts are created in by default."""
    if clear:
        if name:
            raise click.UsageError("--clear takes no repo name")
        repos.set_default_repo(deps.cfg, None)
        click.echo("cleared default repo")
        return
    if name is None:
        current = repos.default_repo(deps.cfg)
        click.echo(current if current else "no default repo")
        return
    try:
        repos.set_default_repo(deps.cfg, name)
    except FileNotFoundError as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"default repo is '{name}'")


@repo.command("rm")
@click.argument("names", nargs=-1, required=True)
@click.pass_obj
def repo_rm(deps: Deps, names: tuple[str, ...]) -> None:
    """Remove registered repositories' mirrors (contexts are untouched)."""
    failed = False
    for name in names:
        try:
            repos.remove_repo(deps.cfg, name)
        except FileNotFoundError as exc:
            click.echo(f"error: {exc}", err=True)
            failed = True
            continue
        click.echo(f"removed '{name}'")
    if failed:
        sys.exit(1)


def main() -> None:
    try:
        cli()
    except subprocess.CalledProcessError as exc:
        cmd = " ".join(map(str, exc.cmd))
        message = f"error: command failed ({cmd})"
        detail = (exc.stderr or "").strip()
        click.echo(f"{message}\n{detail}" if detail else message, err=True)
        sys.exit(exc.returncode or 1)
    except LayoutError as exc:
        click.echo(f"error: invalid layout: {exc}", err=True)
        sys.exit(1)
    except ConfigError as exc:
        click.echo(f"error: invalid config: {exc}", err=True)
        sys.exit(1)
