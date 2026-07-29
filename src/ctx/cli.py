import subprocess
import sys

import click


@click.group()
def cli() -> None:
    """Manage repo-scoped work contexts."""


def main() -> None:
    try:
        cli()
    except subprocess.CalledProcessError as exc:
        cmd = " ".join(map(str, exc.cmd))
        click.echo(f"error: command failed ({cmd})", err=True)
        sys.exit(exc.returncode or 1)
