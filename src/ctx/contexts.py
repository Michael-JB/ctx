import shutil
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


def list_contexts(cfg: Config) -> list[Context]:
    if not cfg.contexts_dir.is_dir():
        return []
    found = []
    for repo_dir in sorted(cfg.contexts_dir.iterdir()):
        if not repo_dir.is_dir():
            continue
        for ctx_dir in sorted(repo_dir.iterdir()):
            if (ctx_dir / ".git").exists():
                found.append(Context(repo_dir.name, ctx_dir.name, ctx_dir))
    return found


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
        git("fetch", "origin", base, cwd=path)
    branch = f"{cfg.branch_prefix}{name}"
    git("checkout", "--no-track", "-b", branch, f"origin/{base}", cwd=path)
    return Context(repo, name, path)


def current_branch(ctx: Context) -> str:
    return git("branch", "--show-current", cwd=ctx.path)


def is_dirty(ctx: Context) -> bool:
    return bool(git("status", "--porcelain", cwd=ctx.path))


def unpushed_commits(ctx: Context) -> list[str]:
    out = git("log", "--branches", "--not", "--remotes", "--oneline", cwd=ctx.path)
    return out.splitlines() if out else []


def remove_context(ctx: Context) -> None:
    shutil.rmtree(ctx.path)
    repo_dir = ctx.path.parent
    if repo_dir.is_dir() and not any(repo_dir.iterdir()):
        repo_dir.rmdir()
