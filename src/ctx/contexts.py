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


def find_context(cfg: Config, ref: str) -> Context:
    """Resolve a 'repo/name' reference to an existing context."""
    if "/" not in ref:
        raise LookupError(f"'{ref}' is not a context reference; use the form repo/name")
    repo, name = ref.split("/", 1)
    path = context_path(cfg, repo, name)
    if not (path / ".git").exists():
        raise LookupError(f"no context '{repo}/{name}'")
    return Context(repo, name, path)


def create_context(cfg: Config, repo: str, name: str) -> Context:
    mirror = repos.repo_path(cfg, repo)
    if not mirror.exists():
        raise FileNotFoundError(f"repo '{repo}' is not registered (ctx repo add <url>)")
    path = context_path(cfg, repo, name)
    if path.exists():
        raise FileExistsError(f"context '{repo}/{name}' already exists at {path}")

    repos.update_repo(cfg, repo)
    default = repos.default_branch(cfg, repo)
    path.parent.mkdir(parents=True, exist_ok=True)
    git("clone", str(mirror), str(path))
    git("remote", "set-url", "origin", repos.repo_url(cfg, repo), cwd=path)
    branch = f"{cfg.branch_prefix}{name}"
    git("checkout", "--no-track", "-b", branch, f"origin/{default}", cwd=path)
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
