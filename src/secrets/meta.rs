//! Names and kinds of secrets, and plain settings, recorded in `bunker.toml` (never secret values).
use crate::config::{Config, SecretKind};
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::paths::{ProjectDir, write_private};
use toml_edit::{DocumentMut, InlineTable, Item, Table, value};

/// What to record for one key.
pub enum Entry<'a> {
    Plain(&'a str),
    Secret(SecretKind),
    Remove,
}

fn table<'d>(d: &'d mut DocumentMut, name: &str) -> &'d mut Table {
    if !d.contains_table(name) {
        d[name] = Item::Table(Table::new());
    }
    d[name].as_table_mut().expect("just made a table")
}

/// Format-preserving edit of `[env]` / `[secrets]` for `key`; the result must validate.
pub fn record(pd: &ProjectDir, key: &str, entry: Entry) -> Result<Config> {
    let path = pd.config();
    let text = std::fs::read_to_string(&path)
        .map_err(|e| BunkerError::io(format!("cannot read {}", path.display()), e))?;
    let mut d: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        BunkerError::new(
            "config_invalid",
            ExitCode::Failure,
            format!("{}: {}", path.display(), e.message()),
        )
    })?;
    table(&mut d, "env").remove(key);
    table(&mut d, "secrets").remove(key);
    match entry {
        Entry::Plain(v) => {
            table(&mut d, "env").insert(key, value(v));
        }
        Entry::Secret(kind) => {
            let mut t = InlineTable::new();
            t.insert(
                "kind",
                match kind {
                    SecretKind::InApp => "in_app",
                    SecretKind::Bound => "bound",
                }
                .into(),
            );
            table(&mut d, "secrets").insert(key, value(t));
        }
        Entry::Remove => {}
    }
    let out = d.to_string();
    let cfg = Config::parse(&out).map_err(|mut e| {
        e.message = e
            .message
            .replacen("bunker.toml", &path.display().to_string(), 1);
        e
    })?;
    if out != text {
        write_private(&path, out.as_bytes())?;
    }
    Ok(cfg)
}
