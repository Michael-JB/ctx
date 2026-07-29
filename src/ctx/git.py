import subprocess
from pathlib import Path


def git(*args: str, cwd: Path | None = None) -> str:
    """Run git, letting stderr (progress, errors) stream to the terminal."""
    result = subprocess.run(
        ["git", *args], cwd=cwd, check=True, stdout=subprocess.PIPE, text=True
    )
    return result.stdout.strip()
