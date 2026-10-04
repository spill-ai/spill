use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Client {
    Cursor,
    Codex,
    #[value(alias = "claude-code")]
    Claude,
}

impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub fn event(self) -> &'static str {
        match self {
            Self::Cursor => "postToolUse",
            _ => "PostToolUse",
        }
    }
}
