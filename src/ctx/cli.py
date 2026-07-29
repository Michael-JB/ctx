import subprocess
import sys

import click

from ctx import contexts, repos
from ctx.config import load_config


@click.group()
def cli() -> None:
    """Manage repo-scoped work contexts."""


@cli.command()
@click.argument("repo")
@click.argument("name")
def new(repo: str, name: str) -> None:
    """Create a context: fresh checkout of REPO on a new local branch."""
    cfg = load_config()
    try:
        ctx = contexts.create_context(cfg, repo, name)
    except (FileExistsError, FileNotFoundError) as exc:
        raise click.ClickException(str(exc))
    click.echo(f"created {ctx.qualified} at {ctx.path} on {contexts.current_branch(ctx)}")


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
        raise click.ClickException(str(exc))
    click.echo(f"registered '{registered}'")


@repo.command("list")
def repo_list() -> None:
    """List registered repositories."""
    cfg = load_config()
    for name in repos.repo_names(cfg):
        click.echo(f"{name}\t{repos.repo_url(cfg, name)}")


@repo.command("remove")
@click.argument("name")
def repo_remove(name: str) -> None:
    """Remove a registered repository's mirror (contexts are untouched)."""
    cfg = load_config()
    try:
        repos.remove_repo(cfg, name)
    except FileNotFoundError as exc:
        raise click.ClickException(str(exc))
    click.echo(f"removed '{name}'")


def main() -> None:
    try:
        cli()
    except subprocess.CalledProcessError as exc:
        cmd = " ".join(map(str, exc.cmd))
        click.echo(f"error: command failed ({cmd})", err=True)
        sys.exit(exc.returncode or 1)
