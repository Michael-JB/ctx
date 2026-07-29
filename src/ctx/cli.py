import subprocess
import sys

import click

from ctx import contexts, repos
from ctx.config import Config, ConfigError, load_config
from ctx.layout import LayoutError
from ctx.multiplexer import Multiplexer, MultiplexerError, get_backend


@click.group()
def cli() -> None:
    """Manage repo-scoped work contexts."""


@cli.command()
@click.argument("repo")
@click.argument("name")
@click.option("-b", "--branch", "base", help="Base branch (default: the repo's default branch).")
def new(repo: str, name: str, base: str | None) -> None:
    """Create a context: fresh checkout of REPO on a new local branch."""
    cfg = load_config()
    try:
        ctx = contexts.create_context(cfg, repo, name, base)
    except (FileExistsError, FileNotFoundError) as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"created {ctx.qualified} at {ctx.path} on {contexts.current_branch(ctx)}")
    try:
        get_backend(cfg.multiplexer, cfg.layout).open(ctx)
    except MultiplexerError as exc:
        raise click.ClickException(str(exc)) from exc


@cli.command("open")
@click.argument("name")
def open_(name: str) -> None:
    """Attach to a context's session, recreating it if needed."""
    cfg = load_config()
    try:
        ctx = contexts.find_context(cfg, name)
    except LookupError as exc:
        raise click.ClickException(str(exc)) from exc
    try:
        get_backend(cfg.multiplexer, cfg.layout).open(ctx)
    except MultiplexerError as exc:
        raise click.ClickException(str(exc)) from exc


@cli.command("list")
def list_() -> None:
    """List contexts with branch, dirtiness, and session state."""
    cfg = load_config()
    all_contexts = contexts.list_contexts(cfg)
    if not all_contexts:
        click.echo("no contexts")
        return
    mux = get_backend(cfg.multiplexer, cfg.layout)
    for ctx in all_contexts:
        branch = contexts.current_branch(ctx)
        flags = []
        if contexts.is_dirty(ctx):
            flags.append("dirty")
        if contexts.unpushed_commits(ctx):
            flags.append("unpushed")
        if mux.exists(ctx):
            flags.append("session")
        click.echo(f"{ctx.qualified}\t{branch}\t{','.join(flags) or '-'}")


@cli.command()
@click.argument("names", nargs=-1, required=True)
@click.option("--force", is_flag=True, help="Delete even with uncommitted or unpushed work.")
def rm(names: tuple[str, ...], force: bool) -> None:
    """Delete contexts: kill their sessions and remove the checkouts."""
    cfg = load_config()
    mux = get_backend(cfg.multiplexer, cfg.layout)
    failed = False
    for name in names:
        error = _remove_one(cfg, mux, name, force)
        if error:
            click.echo(f"error: {error}", err=True)
            failed = True
    if failed:
        sys.exit(1)


def _remove_one(cfg: Config, mux: Multiplexer, name: str, force: bool) -> str | None:
    """Delete one context, returning an error message instead of raising."""
    try:
        ctx = contexts.find_context(cfg, name)
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
    if mux.exists(ctx):
        mux.kill(ctx)
    contexts.remove_context(ctx)
    click.echo(f"removed {ctx.qualified}")
    return None


@cli.group()
def repo() -> None:
    """Manage registered repositories."""


@repo.command("add")
@click.argument("url")
@click.option("--name", help="Registry name (default: derived from URL).")
def repo_add(url: str, name: str | None) -> None:
    """Register a repository by cloning a local bare mirror of it."""
    cfg = load_config()
    try:
        registered = repos.add_repo(cfg, url, name)
    except FileExistsError as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"registered '{registered}'")


@repo.command("list")
def repo_list() -> None:
    """List registered repositories."""
    cfg = load_config()
    for name in repos.repo_names(cfg):
        click.echo(f"{name}\t{repos.repo_url(cfg, name)}")


@repo.command("rm")
@click.argument("name")
def repo_rm(name: str) -> None:
    """Remove a registered repository's mirror (contexts are untouched)."""
    cfg = load_config()
    try:
        repos.remove_repo(cfg, name)
    except FileNotFoundError as exc:
        raise click.ClickException(str(exc)) from exc
    click.echo(f"removed '{name}'")


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
