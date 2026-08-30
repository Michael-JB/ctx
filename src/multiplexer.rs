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
