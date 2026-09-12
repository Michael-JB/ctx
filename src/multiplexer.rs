use std::collections::HashMap;

use crate::contexts::Context;
use crate::layout::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplexerKind {
    Tmux,
    Zellij,
}

impl MultiplexerKind {
    const ALL: &[(&str, MultiplexerKind)] = &[
        ("tmux", MultiplexerKind::Tmux),
        ("zellij", MultiplexerKind::Zellij),
    ];

    pub fn parse(raw: &str) -> Option<MultiplexerKind> {
        Self::ALL
            .iter()
            .find(|(name, _)| *name == raw)
            .map(|(_, kind)| *kind)
    }

    pub fn names() -> String {
        Self::ALL
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct MultiplexerError(pub String);

/// An environment variable as the multiplexers see it: the test env first
/// under tests, the process env otherwise.
pub(crate) fn env_var(key: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(value) = crate::testutil::get_env(key) {
        return Some(value);
    }
    std::env::var(key).ok()
}

/// Python-style truthiness of an env var: set and non-empty.
pub(crate) fn env_truthy(key: &str) -> bool {
    env_var(key).is_some_and(|value| !value.is_empty())
}

pub trait Multiplexer: Send + Sync {
    /// Whether open() returns control instead of taking over the terminal.
    fn can_open_in_place(&self) -> bool;

    /// Whether a session for this context is running.
    fn exists(&self, ctx: &Context) -> bool;

    /// Whether this process runs inside the context's session.
    fn is_current(&self, ctx: &Context) -> bool;

    /// Create the context's session without attaching, if it doesn't exist.
    ///
    /// `values` has open()'s semantics.
    fn create(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError>;

    /// Create the context's session if needed, then attach to it.
    ///
    /// `values` feed the layout's builtin panes and only take effect when
    /// this call creates the session. Pass a mapping (possibly empty) when
    /// opening a freshly created context; None marks a recreated session,
    /// where builtins resume instead of starting anew.
    fn open(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError>;

    /// Tear down the context's session.
    fn kill(&self, ctx: &Context) -> Result<(), MultiplexerError>;
}

/// Records every open on the context, which is what listings order by.
struct MarkOpened<M>(M);

impl<M: Multiplexer> Multiplexer for MarkOpened<M> {
    fn can_open_in_place(&self) -> bool {
        self.0.can_open_in_place()
    }

    fn exists(&self, ctx: &Context) -> bool {
        self.0.exists(ctx)
    }

    fn is_current(&self, ctx: &Context) -> bool {
        self.0.is_current(ctx)
    }

    fn create(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError> {
        self.0.create(ctx, values)
    }

    fn open(
        &self,
        ctx: &Context,
        values: Option<&HashMap<String, String>>,
    ) -> Result<(), MultiplexerError> {
        // Before, not after: a terminal-takeover attach only returns on detach.
        crate::contexts::mark_opened(ctx);
        self.0.open(ctx, values)
    }

    fn kill(&self, ctx: &Context) -> Result<(), MultiplexerError> {
        self.0.kill(ctx)
    }
}

pub fn get_multiplexer(kind: MultiplexerKind, layout: Node) -> std::sync::Arc<dyn Multiplexer> {
    match kind {
        MultiplexerKind::Tmux => std::sync::Arc::new(MarkOpened(
            crate::multiplexers::tmux::TmuxMultiplexer::new(layout),
        )),
        MultiplexerKind::Zellij => std::sync::Arc::new(MarkOpened(
            crate::multiplexers::zellij::ZellijMultiplexer::new(layout),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Inert;

    impl Multiplexer for Inert {
        fn can_open_in_place(&self) -> bool {
            true
        }
        fn exists(&self, _ctx: &Context) -> bool {
            false
        }
        fn is_current(&self, _ctx: &Context) -> bool {
            false
        }
        fn create(
            &self,
            _ctx: &Context,
            _values: Option<&HashMap<String, String>>,
        ) -> Result<(), MultiplexerError> {
            Ok(())
        }
        fn open(
            &self,
            _ctx: &Context,
            _values: Option<&HashMap<String, String>>,
        ) -> Result<(), MultiplexerError> {
            Ok(())
        }
        fn kill(&self, _ctx: &Context) -> Result<(), MultiplexerError> {
            Ok(())
        }
    }

    #[test]
    fn open_marks_the_context_as_opened() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let ctx = Context {
            repo: "repo".into(),
            name: "a".into(),
            path: PathBuf::from(dir.path()),
        };

        MarkOpened(Inert).open(&ctx, None).unwrap();

        assert!(ctx.path.join(".git").join("ctx-opened").exists());
    }
}
