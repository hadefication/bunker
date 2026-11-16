//! Human output by default, a stable JSON envelope with `--json`, and the exit codes.
use crate::error::BunkerError;
use serde_json::{Value, json};
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Ok = 0,
    Failure = 1,
    Usage = 2,
    NotFound = 3,
    Refused = 4,
    Unconfined = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

pub trait Report {
    fn human(&self, w: &mut dyn Write) -> io::Result<()>;
    fn json(&self) -> Value;
    /// Exit code for a report that succeeded but describes a problem (status: degraded, unconfined).
    fn exit(&self) -> ExitCode {
        ExitCode::Ok
    }
}

/// Text from outside bunker (guest logs, a visitor's URL, a guest command's output) with control
/// characters escaped, so it can't move the cursor, retitle the window or hide lines in a
/// terminal. Tabs stay.
pub fn printable(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.chars().any(|c| c.is_control() && c != '\t') {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(
        s.chars()
            .map(|c| {
                if c.is_control() && c != '\t' {
                    c.escape_unicode().to_string()
                } else {
                    c.to_string()
                }
            })
            .collect(),
    )
}

pub fn ok_envelope(data: Value) -> Value {
    json!({"ok": true, "data": data})
}

pub fn error_envelope(e: &BunkerError) -> Value {
    json!({"ok": false, "error": {"code": e.code, "message": e.message, "hint": e.hint}})
}

/// Prints a report to stdout and returns its exit code.
pub fn emit(format: Format, r: &dyn Report) -> ExitCode {
    let mut out = io::stdout().lock();
    let written = match format {
        Format::Human => r.human(&mut out),
        Format::Json => writeln!(out, "{}", ok_envelope(r.json())),
    };
    match written {
        Ok(()) => r.exit(),
        Err(_) => ExitCode::Failure,
    }
}

/// Human errors go to stderr with the hint and a pointer to `--json`; JSON errors go to stdout.
pub fn emit_error(format: Format, e: &BunkerError) -> ExitCode {
    match format {
        Format::Json => {
            let _ = writeln!(io::stdout().lock(), "{}", error_envelope(e));
        }
        Format::Human => {
            let mut err = io::stderr().lock();
            let _ = writeln!(err, "error: {}", e.message);
            if let Some(h) = &e.hint {
                let _ = writeln!(err, "  hint: {h}");
            }
            let _ = writeln!(err, "  (for scripts: add --json)");
        }
    }
    e.exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_json_has_code_message_hint() {
        let e = BunkerError::new("x", ExitCode::Refused, "m").hint("h");
        assert_eq!(
            error_envelope(&e),
            json!({"ok": false, "error": {"code": "x", "message": "m", "hint": "h"}})
        );
        let bare = BunkerError::new("y", ExitCode::Failure, "n");
        assert_eq!(
            error_envelope(&bare),
            json!({"ok": false, "error": {"code": "y", "message": "n", "hint": null}})
        );
    }

    #[test]
    fn ok_json_wraps_data() {
        assert_eq!(
            ok_envelope(json!({"a": 1})),
            json!({"ok": true, "data": {"a": 1}})
        );
    }
}
