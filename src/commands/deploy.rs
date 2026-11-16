//! `bunker deploy <project>`: ship the repo's HEAD (or, with --dirty, its tracked working tree) as a
//! new release. The pipeline lives in `crate::deploy`.
pub use crate::deploy::run::{DeployOpts, DeployReport, StepResult, deploy};

/// One human line per finished step, printed as the deploy goes.
pub fn step_line(s: &StepResult) -> String {
    format!(
        "  {} {:<20} {:>6.1}s",
        if s.ok { "ok  " } else { "FAIL" },
        s.name,
        s.secs
    )
}
