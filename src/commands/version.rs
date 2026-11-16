use crate::cli::VERSION;
use crate::output::Report;
use serde_json::{Value, json};
use std::io::{self, Write};

pub struct VersionReport;

impl Report for VersionReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "bunker {VERSION}")
    }

    fn json(&self) -> Value {
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "commit": env!("BUNKER_COMMIT"),
            "build_date": env!("BUNKER_BUILD_DATE"),
        })
    }
}
