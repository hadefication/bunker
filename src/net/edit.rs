//! Format-preserving edits to `[network]` lists, refused if the result is not a valid config.
use crate::config::Config;
use crate::error::Result;
use serde::Serialize;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ListName {
    Allow,
    Build,
    Deny,
}

impl ListName {
    pub fn key(self) -> &'static str {
        match self {
            ListName::Allow => "allow",
            ListName::Build => "build",
            ListName::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub list: ListName,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// Hosts compare case-insensitively with any trailing dot removed; entries are stored that way.
pub fn normalize_host(h: &str) -> String {
    let h = h.trim().to_ascii_lowercase();
    h.strip_suffix('.').map(str::to_string).unwrap_or(h)
}

/// Applies `edits` in order, keeping comments and key order. The result must pass `Config::parse`.
pub fn edit_lists(doc: &str, edits: &[(ListName, Op, &str)]) -> Result<(String, Vec<Change>)> {
    let mut d: DocumentMut = doc.parse().map_err(|e: toml_edit::TomlError| {
        crate::error::BunkerError::new(
            "config_invalid",
            crate::output::ExitCode::Failure,
            format!("bunker.toml: {}", e.message()),
        )
    })?;
    if !d.contains_table("network") {
        d["network"] = Item::Table(Table::new());
    }
    let mut changes = Vec::new();
    for &(list, op, host) in edits {
        let host = normalize_host(host);
        let net = d["network"].as_table_mut().ok_or_else(|| {
            crate::error::BunkerError::new(
                "config_invalid",
                crate::output::ExitCode::Failure,
                "bunker.toml: network must be a table",
            )
        })?;
        let item = net
            .entry(list.key())
            .or_insert(toml_edit::value(Array::new()));
        let arr = item.as_array_mut().ok_or_else(|| {
            crate::error::BunkerError::new(
                "config_invalid",
                crate::output::ExitCode::Failure,
                format!("bunker.toml: network.{} must be a list", list.key()),
            )
        })?;
        let before: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let present = |v: &Value| v.as_str().is_some_and(|s| normalize_host(s) == host);
        match op {
            Op::Add if !arr.iter().any(present) => arr.push(host.as_str()),
            Op::Remove => arr.retain(|v| !present(v)),
            Op::Add => {}
        }
        let after: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        changes.push(Change {
            list,
            before,
            after,
        });
    }
    let out = d.to_string();
    Config::parse(&out)?;
    Ok((out, changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, fixture_config};

    fn commented() -> String {
        fixture_config().replace(
            "[network]",
            "# keep me: above network\n[network]  # keep me too",
        )
    }

    #[test]
    fn allow_preserves_comments_and_order() {
        let before = commented();
        let (after, changes) =
            edit_lists(&before, &[(ListName::Allow, Op::Add, "api.stripe.com")]).unwrap();
        let mut rest = after.lines();
        for line in before.lines().filter(|l| !l.starts_with("allow")) {
            assert!(
                rest.any(|a| a == line),
                "lost or reordered: {line}\n---\n{after}"
            );
        }
        assert!(after.contains("# keep me: above network"));
        assert!(after.contains("allow = [\"api.stripe.com\"]"), "{after}");
        assert!(
            after.contains("# hosts the running site may reach"),
            "value comment kept: {after}"
        );
        assert_eq!(
            Config::parse(&after).unwrap().network.allow,
            ["api.stripe.com"]
        );
        assert_eq!(
            changes[0],
            Change {
                list: ListName::Allow,
                before: vec![],
                after: vec!["api.stripe.com".into()]
            }
        );
    }

    #[test]
    fn allow_is_idempotent() {
        let (once, _) = edit_lists(
            &fixture_config(),
            &[(ListName::Allow, Op::Add, "a.example.com")],
        )
        .unwrap();
        let (twice, changes) =
            edit_lists(&once, &[(ListName::Allow, Op::Add, "A.Example.com")]).unwrap();
        assert_eq!(once, twice);
        assert!(changes.iter().all(|c| c.before == c.after));
    }

    #[test]
    fn deny_removes_from_allow_and_build() {
        let (doc, _) = edit_lists(
            &fixture_config(),
            &[
                (ListName::Allow, Op::Add, "x.example.com"),
                (ListName::Build, Op::Add, "x.example.com"),
            ],
        )
        .unwrap();
        let (doc, changes) = edit_lists(
            &doc,
            &[
                (ListName::Deny, Op::Add, "x.example.com"),
                (ListName::Allow, Op::Remove, "x.example.com"),
                (ListName::Build, Op::Remove, "x.example.com"),
            ],
        )
        .unwrap();
        let n = Config::parse(&doc).unwrap().network;
        assert_eq!(
            (n.allow.len(), n.build.len(), n.deny),
            (0, 0, vec!["x.example.com".to_string()])
        );
        assert_eq!(changes.len(), 3);
    }

    #[test]
    fn missing_network_table_is_created() {
        let doc = fixture_config();
        let cut = doc.find("[network]").unwrap();
        let (after, _) =
            edit_lists(&doc[..cut], &[(ListName::Deny, Op::Add, "evil.example")]).unwrap();
        assert_eq!(
            Config::parse(&after).unwrap().network.deny,
            ["evil.example"]
        );
    }
}
