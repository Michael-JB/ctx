import subprocess
import sys
from dataclasses import dataclass

import click

from ctx import contexts, repos
from ctx.config import Config, ConfigError, load_config
from ctx.layout import LayoutError
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
@click.argument("name")
@click.option("-b", "--branch", "base", help="Base branch (default: the repo's default branch).")
@click.pass_obj
def new(deps: Deps, repo: str, name: str, base: str | None) -> None:
    """Create a context: fresh checkout of REPO on a new local branch."""
    _create_and_open(deps, repo, name, base)


def _create_and_open(deps: Deps, repo: str, name: str, base: str | None) -> None:
    try:
        ctx = contexts.create_context(deps.cfg, repo, name, base)
    except (FileExistsError, FileNotFoundError) as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"created {ctx.qualified} at {ctx.path} on {contexts.current_branch(ctx)}")
    try:
        deps.mux.open(ctx)
    except MultiplexerError as exc:
        raise click.ClickException(str(exc)) from exc


@cli.command("open")
@click.argument("name")
@click.pass_obj
def open_(deps: Deps, name: str) -> None:
    """Attach to a context's session, recreating it if needed."""
    try:
        ctx = contexts.find_context(deps.cfg, name)
    except LookupError as exc:
        raise click.ClickException(str(exc)) from exc
    try:
        deps.mux.open(ctx)
    except MultiplexerError as exc:
        raise click.ClickException(str(exc)) from exc


@cli.command("list")
@click.pass_obj
def list_(deps: Deps) -> None:
    """List contexts with branch, dirtiness, and session state."""
    all_contexts = contexts.list_contexts(deps.cfg)
    if not all_contexts:
        click.echo("no contexts")
        return
    rows = [("NAME", "REPO", "BRANCH", "STATUS")]
    for ctx in all_contexts:
        status = []
        if contexts.is_dirty(ctx):
            status.append("uncommitted changes")
        unpushed = contexts.unpushed_commits(ctx)
        if unpushed:
            status.append(f"{len(unpushed)} unpushed commit(s)")
        rows.append(
            (
                ctx.name,
                ctx.repo,
                contexts.current_branch(ctx),
                ", ".join(status) or "clean",
            )
        )
    widths = [max(len(row[column]) for row in rows) for column in range(len(rows[0]))]
    for row in rows:
        cells = zip(row, widths, strict=True)
        click.echo("  ".join(cell.ljust(width) for cell, width in cells).rstrip())


@cli.command()
@click.argument("names", nargs=-1, required=True)
@click.option("--force", is_flag=True, help="Delete even with uncommitted or unpushed work.")
@click.pass_obj
def rm(deps: Deps, names: tuple[str, ...], force: bool) -> None:
    """Delete contexts: kill their sessions and remove the checkouts."""
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
        ctx = contexts.find_context(deps.cfg, name)
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
    if deps.mux.exists(ctx):
        deps.mux.kill(ctx)
    contexts.remove_context(ctx)
    click.echo(f"removed {ctx.qualified}")
    return None


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
            _create_and_open(deps, repo_name, name, base)
        case None:
            pass


@cli.group()
def repo() -> None:
    """Manage registered repositories."""


@repo.command("add")
@click.argument("url")
@click.option("--name", help="Registry name (default: derived from URL).")
@click.pass_obj
def repo_add(deps: Deps, url: str, name: str | None) -> None:
    """Register a repository by cloning a local bare mirror of it."""
    try:
        registered = repos.add_repo(deps.cfg, url, name)
    except FileExistsError as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"registered '{registered}'")


@repo.command("list")
@click.pass_obj
def repo_list(deps: Deps) -> None:
    """List registered repositories."""
    for name in repos.repo_names(deps.cfg):
        click.echo(f"{name}\t{repos.repo_url(deps.cfg, name)}")


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
        click.echo(f"error: command failed ({cmd})", err=True)
        sys.exit(exc.returncode or 1)
    except LayoutError as exc:
        click.echo(f"error: invalid layout: {exc}", err=True)
        sys.exit(1)
    except ConfigError as exc:
        click.echo(f"error: invalid config: {exc}", err=True)
        sys.exit(1)
