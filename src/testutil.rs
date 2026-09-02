//! Test support: process isolation and repo fixtures for the inline tests.
//!
//! Rust tests share the process and run in parallel, so the Python suite's
//! environment monkeypatching is off the table. Instead, every subprocess
//! the crate spawns is built through `git::new_command`, which under
//! `cfg(test)` routes through `apply_test_env`: a process-wide isolated git
//! config (identity, main as the default branch) plus per-thread extras a
//! test can push (e.g. a PATH with stub executables).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use crate::config::Config;
use crate::git::git_quiet;

static GIT_ISOLATION: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();

fn isolated_gitconfig() -> &'static Path {
    let (_, path) = GIT_ISOLATION.get_or_init(|| {
        let dir = tempfile::tempdir().expect("create git isolation dir");
        let path = dir.path().join("gitconfig");
        std::fs::write(
            &path,
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
        )
        .expect("write isolated gitconfig");
        (dir, path)
    });
    path
}

thread_local! {
    static EXTRA_ENV: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Point a spawned command at the isolated git config and the calling
/// test's extra environment.
pub fn apply_test_env(cmd: &mut Command) {
    cmd.env("GIT_CONFIG_GLOBAL", isolated_gitconfig());
    cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
    EXTRA_ENV.with(|extra| {
        for (key, value) in extra.borrow().iter() {
            cmd.env(key, value);
        }
    });
}

/// Set an environment variable for commands spawned from this thread,
/// undone when the guard drops.
pub fn push_env(key: &str, value: &str) -> EnvGuard {
    EXTRA_ENV.with(|extra| {
        extra
            .borrow_mut()
            .push((key.to_string(), value.to_string()));
    });
    EnvGuard
}

/// The calling test's value for a variable, if it pushed one (latest wins).
pub fn get_env(key: &str) -> Option<String> {
    EXTRA_ENV.with(|extra| {
        extra
            .borrow()
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    })
}

/// Overlay the calling test's pushed variables onto an environment map.
pub fn overlay_env(env: &mut std::collections::BTreeMap<String, String>) {
    EXTRA_ENV.with(|extra| {
        for (key, value) in extra.borrow().iter() {
            env.insert(key.clone(), value.clone());
        }
    });
}

/// Carry the calling test's pushed variables into a worker thread's closure.
pub fn propagate_env<R, F: FnOnce() -> R + Send>(f: F) -> impl FnOnce() -> R + Send {
    let snapshot: Vec<(String, String)> = EXTRA_ENV.with(|extra| extra.borrow().clone());
    move || {
        EXTRA_ENV.with(|extra| *extra.borrow_mut() = snapshot);
        f()
    }
}

pub struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        EXTRA_ENV.with(|extra| {
            extra.borrow_mut().pop();
        });
    }
}

/// Run git in a fixture, panicking on failure.
pub fn git(args: &[&str], cwd: &Path) -> String {
    git_quiet(args, Some(cwd)).expect("fixture git call succeeds")
}

/// A Config rooted in a fresh temp dir, which also hosts origin fixtures.
pub struct TestEnv {
    pub cfg: Config,
    dir: tempfile::TempDir,
}

impl TestEnv {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// A local repo standing in for a remote origin.
    pub fn make_origin(&self, name: &str, empty: bool) -> PathBuf {
        let path = self.root().join(name);
        std::fs::create_dir(&path).unwrap();
        git(&["init"], &path);
        if !empty {
            commit_file(&path, "README.md", "hello\n");
        }
        path
    }

    pub fn origin(&self) -> PathBuf {
        self.make_origin("origin", false)
    }

    /// Shadow an executable on PATH with a stub script for this thread.
    pub fn fake_cli(&self, name: &str, script: &str) -> EnvGuard {
        use std::os::unix::fs::PermissionsExt;

        let bin_dir = self.root().join("bin");
        let _ = std::fs::create_dir(&bin_dir);
        let stub = bin_dir.join(name);
        std::fs::write(&stub, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        push_env("PATH", &format!("{}:{path}", bin_dir.display()))
    }
}

pub fn test_env() -> TestEnv {
    let dir = tempfile::tempdir().expect("create test dir");
    let cfg = Config {
        contexts_dir: dir.path().join("contexts"),
        repos_dir: dir.path().join("repos"),
        archive_dir: dir.path().join("archive"),
        // Freshness would turn a test's later fetches into silent no-ops;
        // the tests covering it opt back in with a window of their own.
        mirror_max_age: 0.0,
        ..Config::default()
    };
    TestEnv { cfg, dir }
}

/// Backdate a mirror's last-fetch stamp, making it stale for any freshness window.
pub fn age_fetch_stamp(cfg: &Config, repo: &str, age: std::time::Duration) {
    let stamp = crate::repos::repo_path(cfg, repo).join("FETCH_HEAD");
    let times = std::fs::FileTimes::new().set_modified(std::time::SystemTime::now() - age);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&stamp)
        .expect("mirror has a fetch stamp")
        .set_times(times)
        .expect("backdate fetch stamp");
}

pub fn commit_file(repo: &Path, name: &str, content: &str) {
    std::fs::write(repo.join(name), content).unwrap();
    git(&["add", name], repo);
    git(&["commit", "-m", &format!("add {name}")], repo);
}

/// Whether git-lfs is installed; tests that need it return early otherwise.
pub fn lfs_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        git_quiet(&["lfs", "version"], None).is_ok() || {
            eprintln!("git-lfs not installed; skipping");
            false
        }
    })
}

/// Commit a file tracked by git-lfs, installing its filters in the isolated config.
pub fn commit_lfs_file(repo: &Path, name: &str, content: &str) {
    // `git lfs install` rewrites the shared isolated config; serialize against
    // parallel tests racing its lockfile.
    static INSTALL: Mutex<()> = Mutex::new(());
    {
        let _lock = INSTALL.lock().unwrap();
        git(&["lfs", "install"], repo);
    }
    git(&["lfs", "track", name], repo);
    std::fs::write(repo.join(name), content).unwrap();
    git(&["add", ".gitattributes", name], repo);
    git(&["commit", "-m", &format!("add {name}")], repo);
}
