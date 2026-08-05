import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

from ctx import repos
from ctx.config import Config
from ctx.git import git


@dataclass(frozen=True)
class Context:
    repo: str
    name: str
    path: Path

    @property
    def qualified(self) -> str:
        return f"{self.repo}/{self.name}"


def context_path(cfg: Config, repo: str, name: str) -> Path:
    return cfg.contexts_dir / repo / name


def last_active(ctx: Context) -> float:
    """Proxy for the last interaction: the latest git activity in the checkout.

    `.git/logs/HEAD` is appended to on commits, checkouts, and resets;
    `.git/index` is rewritten by staging and status refreshes. Neither sees
    plain file edits, but agent-driven work touches git constantly.
    """
    candidates = (ctx.path / ".git" / "logs" / "HEAD", ctx.path / ".git" / "index")
    times = [path.stat().st_mtime for path in candidates if path.exists()]
    return max(times, default=ctx.path.stat().st_mtime)


def _scan(root: Path) -> list[Context]:
    """Contexts under a <root>/<repo>/<name> tree, most recently active first."""
    if not root.is_dir():
        return []
    found = []
    for repo_dir in sorted(root.iterdir()):
        if not repo_dir.is_dir():
            continue
        for ctx_dir in sorted(repo_dir.iterdir()):
            if (ctx_dir / ".git").exists():
                found.append(Context(repo_dir.name, ctx_dir.name, ctx_dir))
    found.sort(key=lambda c: (-last_active(c), c.qualified))
    return found


def list_contexts(cfg: Config) -> list[Context]:
    """All contexts, most recently active first."""
    return _scan(cfg.contexts_dir)


def find_context(cfg: Config, name: str) -> Context:
    """Resolve a context name; names are globally unique."""
    matches = [c for c in list_contexts(cfg) if c.name == name]
    if not matches:
        raise LookupError(f"no context '{name}'")
    return matches[0]


def create_context(cfg: Config, repo: str, name: str, base: str | None = None) -> Context:
    mirror = repos.repo_path(cfg, repo)
    if not mirror.exists():
        raise FileNotFoundError(f"repo '{repo}' is not registered (ctx repo add <url>)")
    taken = next((c for c in list_contexts(cfg) if c.name == name), None)
    if taken is not None:
        raise FileExistsError(f"context name '{name}' is already used by {taken.qualified}")
    path = context_path(cfg, repo, name)

    if base is None:
        repos.update_repo(cfg, repo)
        base = repos.default_branch(cfg, repo)
        fetch_base = False
    else:
        # The mirror only carries the default branch; fetch the base into the context.
        fetch_base = True
    path.parent.mkdir(parents=True, exist_ok=True)
    git("clone", str(mirror), str(path))
    git("remote", "set-url", "origin", repos.repo_url(cfg, repo), cwd=path)
    if fetch_base:
        try:
            git("fetch", "origin", base, cwd=path)
        except subprocess.CalledProcessError as exc:
            remove_context(Context(repo, name, path))
            raise FileNotFoundError(f"branch '{base}' not found on origin of '{repo}'") from exc
    branch = f"{cfg.branch_prefix}{name}"
    if git("for-each-ref", f"refs/remotes/origin/{base}", cwd=path):
        git("checkout", "--no-track", "-b", branch, f"origin/{base}", cwd=path)
    else:
        # An empty repo has no commit to branch from; start the work branch unborn.
        git("checkout", "--no-track", "-b", branch, cwd=path)
    return Context(repo, name, path)


def archive_path(cfg: Config, repo: str, name: str) -> Path:
    return cfg.archive_dir / repo / name


def list_archived(cfg: Config) -> list[Context]:
    """All archived contexts, most recently active first."""
    return _scan(cfg.archive_dir)


def find_archived(cfg: Config, name: str) -> Context:
    """Resolve an archived context by name."""
    matches = [c for c in list_archived(cfg) if c.name == name]
    if not matches:
        raise LookupError(f"no archived context '{name}'")
    return matches[0]


def archive_context(cfg: Config, ctx: Context) -> Context:
    """Move a context's checkout into the archive."""
    dest = archive_path(cfg, ctx.repo, ctx.name)
    if dest.exists():
        raise FileExistsError(f"'{ctx.qualified}' is already archived")
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(ctx.path, dest)
    return Context(ctx.repo, ctx.name, dest)


def current_branch(ctx: Context) -> str:
    return git("branch", "--show-current", cwd=ctx.path)


def is_dirty(ctx: Context) -> bool:
    return bool(git("status", "--porcelain", cwd=ctx.path))


def unpushed_commits(ctx: Context) -> list[str]:
    out = git("log", "--branches", "--not", "--remotes", "--oneline", cwd=ctx.path)
    return out.splitlines() if out else []


def remove_context(ctx: Context) -> None:
    shutil.rmtree(ctx.path)
