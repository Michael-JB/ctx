import asyncio
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

from ctx import repos
from ctx.config import Config
from ctx.git import git, git_async


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


def _check_name_free(cfg: Config, name: str, *, exclude: Path | None = None) -> None:
    """Names are unique across live and archived contexts alike.

    Sharing a name with an archived context would leave that archive
    unrestorable, so the two pools compete for the same names.
    """
    for ctx in list_contexts(cfg):
        if ctx.name == name and ctx.path != exclude:
            raise FileExistsError(f"context name '{name}' is already used by {ctx.qualified}")
    for ctx in list_archived(cfg):
        if ctx.name == name and ctx.path != exclude:
            raise FileExistsError(
                f"context name '{name}' is already used by archived {ctx.qualified}"
            )


def _check_name(name: str, branch: str) -> None:
    """Reject names that break the paths, branches, or commands they feed."""
    if not name.strip():
        raise ValueError("context name must not be empty")
    if "/" in name or name in {".", ".."}:
        raise ValueError(f"context name '{name}' must be a single path component")
    if name.startswith("-"):
        raise ValueError(f"context name '{name}' must not start with '-'")
    # cwd="/": the process's own cwd may have been deleted under it.
    check = subprocess.run(
        ["git", "check-ref-format", f"refs/heads/{branch}"], capture_output=True, cwd="/"
    )
    if check.returncode != 0:
        raise ValueError(f"context name '{name}' does not make a valid branch name ('{branch}')")


async def create_context(cfg: Config, repo: str, name: str, base: str | None = None) -> Context:
    # Spaces are welcome in context names but not in branch names; dash them
    # out. Anything else unfit for a branch is rejected, not rewritten.
    branch = cfg.branch_prefix + name.replace(" ", "-")
    _check_name(name, branch)
    mirror = repos.repo_path(cfg, repo)
    if not mirror.exists():
        raise FileNotFoundError(f"repo '{repo}' is not registered (ctx repo add <url>)")
    _check_name_free(cfg, name)
    path = context_path(cfg, repo, name)

    if base is None:
        await repos.update_repo(cfg, repo)
        base = await repos.default_branch(cfg, repo)
        fetch_base = False
    else:
        # The mirror only carries the default branch; fetch the base into the context.
        fetch_base = True
    path.parent.mkdir(parents=True, exist_ok=True)
    # A directory that predates the clone is not ours to delete on failure.
    preexisting = path.exists()
    try:
        await git_async("clone", str(mirror), str(path))
        await git_async("remote", "set-url", "origin", repos.repo_url(cfg, repo), cwd=path)
        if fetch_base:
            try:
                await git_async("fetch", "origin", base, cwd=path)
            except subprocess.CalledProcessError as exc:
                remove_context(Context(repo, name, path))
                raise FileNotFoundError(f"branch '{base}' not found on origin of '{repo}'") from exc
        if await git_async("for-each-ref", f"refs/remotes/origin/{base}", cwd=path):
            await git_async("checkout", "--no-track", "-b", branch, f"origin/{base}", cwd=path)
        else:
            # An empty repo has no commit to branch from; start the work branch unborn.
            await git_async("checkout", "--no-track", "-b", branch, cwd=path)
    except (asyncio.CancelledError, subprocess.CalledProcessError):
        # A half-made checkout would squat on the name; leave nothing behind.
        if not preexisting:
            shutil.rmtree(path, ignore_errors=True)
        raise
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


def find_any(cfg: Config, name: str) -> Context:
    """Resolve a context name among live and archived contexts alike."""
    matches = [c for c in list_contexts(cfg) + list_archived(cfg) if c.name == name]
    if not matches:
        raise LookupError(f"no context '{name}'")
    return matches[0]


def is_archived(cfg: Config, ctx: Context) -> bool:
    return ctx.path.is_relative_to(cfg.archive_dir)


def archive_context(cfg: Config, ctx: Context) -> Context:
    """Move a context's checkout into the archive."""
    _check_name_free(cfg, ctx.name, exclude=ctx.path)
    dest = archive_path(cfg, ctx.repo, ctx.name)
    if dest.exists():
        raise FileExistsError(f"'{ctx.qualified}' is already archived")
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(ctx.path, dest)
    return Context(ctx.repo, ctx.name, dest)


def unarchive_context(cfg: Config, ctx: Context) -> Context:
    """Move an archived checkout back among the live contexts."""
    _check_name_free(cfg, ctx.name, exclude=ctx.path)
    dest = context_path(cfg, ctx.repo, ctx.name)
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(ctx.path, dest)
    return Context(ctx.repo, ctx.name, dest)


def current_branch(ctx: Context) -> str:
    """The checkout's branch, read from `.git/HEAD` to spare a subprocess.

    Anything but a symbolic ref to a branch (e.g. a detached HEAD's raw
    hash) reads as no branch, like `git branch --show-current`.
    """
    head = (ctx.path / ".git" / "HEAD").read_text().strip()
    prefix = "ref: refs/heads/"
    return head.removeprefix(prefix) if head.startswith(prefix) else ""


def is_dirty(ctx: Context) -> bool:
    return bool(git("status", "--porcelain", cwd=ctx.path))


def unpushed_commits(ctx: Context) -> list[str]:
    out = git("log", "--branches", "--not", "--remotes", "--oneline", cwd=ctx.path)
    return out.splitlines() if out else []


def remove_context(ctx: Context) -> None:
    shutil.rmtree(ctx.path)


def empty_archive(cfg: Config) -> None:
    """Permanently delete every archived context."""
    for ctx in list_archived(cfg):
        remove_context(ctx)
