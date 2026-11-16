//! The one error type every command returns.
use crate::output::ExitCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BunkerError {
    /// Stable snake_case identifier, the `error.code` in `--json`.
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
    pub exit: ExitCode,
}

impl BunkerError {
    pub fn new(code: &'static str, exit: ExitCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            exit,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// An unexpected I/O failure: exit 1.
    pub fn io(what: impl std::fmt::Display, err: std::io::Error) -> Self {
        Self::new("io", ExitCode::Failure, format!("{what}: {err}"))
    }
}

impl std::fmt::Display for BunkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BunkerError {}

pub type Result<T> = std::result::Result<T, BunkerError>;
