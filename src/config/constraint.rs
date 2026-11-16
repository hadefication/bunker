//! The highest supported PHP version a Composer `require.php` constraint allows.
use crate::config::validate::SUPPORTED_PHP;
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;

type V = (u32, u32, u32);

#[derive(Debug, Clone, Copy)]
enum Op {
    Ge,
    Gt,
    Le,
    Lt,
}

/// One AND group: every bound must hold.
type Group = Vec<(Op, V)>;

/// The highest version in `SUPPORTED_PHP` with some patch release that satisfies `c`. Handles `^`,
/// `~`, `>=`, `>`, `<=`, `<`, exact and wildcard (`8.2.*`) versions, `||`, and AND by space or comma.
pub fn php_from_constraint(c: &str) -> Result<String> {
    let groups = parse(c).ok_or_else(|| {
        err(format!(
            "cannot read the PHP constraint {c:?} in composer.json"
        ))
    })?;
    SUPPORTED_PHP
        .iter()
        .rev()
        .find(|v| {
            let (maj, min) = v.split_once('.').expect("SUPPORTED_PHP is major.minor");
            let (maj, min) = (maj.parse().unwrap(), min.parse().unwrap());
            (0..100).any(|patch| groups.iter().any(|g| holds(g, (maj, min, patch))))
        })
        .map(|v| v.to_string())
        .ok_or_else(|| err(format!("no supported PHP version satisfies {c:?}")))
}

fn err(msg: String) -> BunkerError {
    BunkerError::new("php_constraint", ExitCode::Refused, msg).hint("pass --php <version>")
}

fn holds(g: &Group, v: V) -> bool {
    g.iter().all(|&(op, b)| match op {
        Op::Ge => v >= b,
        Op::Gt => v > b,
        Op::Le => v <= b,
        Op::Lt => v < b,
    })
}

fn parse(c: &str) -> Option<Vec<Group>> {
    let c = c.trim();
    if c.is_empty() {
        return None;
    }
    c.split("||")
        .map(|alt| {
            let terms: Vec<&str> = alt.split([' ', ',']).filter(|t| !t.is_empty()).collect();
            if terms.is_empty() {
                return None;
            }
            let mut g = Group::new();
            for t in terms {
                g.extend(term(t)?);
            }
            Some(g)
        })
        .collect()
}

/// Parsed version: the numbers given (1-3 of them) and whether the last part was a `*`.
fn version(s: &str) -> Option<(Vec<u32>, bool)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts: Vec<&str> = s.split('.').collect();
    let wildcard = parts.last() == Some(&"*");
    if wildcard {
        parts.pop();
    }
    if parts.len() > 3 || (parts.is_empty() && !wildcard) {
        return None;
    }
    let nums = parts
        .iter()
        .map(|p| {
            (!p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                .then(|| p.parse().ok())
                .flatten()
        })
        .collect::<Option<Vec<u32>>>()?;
    Some((nums, wildcard))
}

fn full(n: &[u32]) -> V {
    (
        n.first().copied().unwrap_or(0),
        n.get(1).copied().unwrap_or(0),
        n.get(2).copied().unwrap_or(0),
    )
}

/// The exclusive upper bound after bumping the part at `idx` (0 major, 1 minor).
fn bump(n: &[u32], idx: usize) -> V {
    match idx {
        0 => (n[0] + 1, 0, 0),
        _ => (n[0], n.get(1).copied().unwrap_or(0) + 1, 0),
    }
}

fn term(t: &str) -> Option<Group> {
    for (prefix, op) in [(">=", Op::Ge), ("<=", Op::Le), (">", Op::Gt), ("<", Op::Lt)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let (n, wild) = version(rest)?;
            return (!wild && !n.is_empty()).then(|| vec![(op, full(&n))]);
        }
    }
    if let Some(rest) = t.strip_prefix('^') {
        let (n, wild) = version(rest)?;
        if wild || n.is_empty() {
            return None;
        }
        // ^ keeps the first non-zero part fixed.
        let idx = if n[0] > 0 || n.len() == 1 { 0 } else { 1 };
        return Some(vec![(Op::Ge, full(&n)), (Op::Lt, bump(&n, idx))]);
    }
    if let Some(rest) = t.strip_prefix('~') {
        let (n, wild) = version(rest)?;
        if wild || n.is_empty() {
            return None;
        }
        // ~8 and ~8.2 allow up to the next major; ~8.2.1 up to the next minor.
        let idx = if n.len() == 3 { 1 } else { 0 };
        return Some(vec![(Op::Ge, full(&n)), (Op::Lt, bump(&n, idx))]);
    }
    let rest = t.strip_prefix('=').unwrap_or(t);
    let (n, wild) = version(rest)?;
    if wild {
        return Some(match n.len() {
            0 => vec![],
            1 | 2 => vec![(Op::Ge, full(&n)), (Op::Lt, bump(&n, n.len() - 1))],
            _ => return None,
        });
    }
    Some(vec![(Op::Ge, full(&n)), (Op::Le, full(&n))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_from_constraint_table() {
        for (c, want) in [
            ("^8.3", "8.5"),
            ("~7.4", "7.4"),
            (">=8.1 <8.3", "8.2"),
            (">=8.1,<8.3", "8.2"),
            ("^7.4 || ^8.0", "8.5"),
            ("8.2.*", "8.2"),
            ("8.1", "8.1"),
            ("~8.2.0", "8.2"),
            (">8.4", "8.5"),
            ("<=7.2", "7.2"),
            ("^7.1.3", "7.4"),
            ("*", "8.5"),
            ("v8.3.0", "8.3"),
        ] {
            assert_eq!(php_from_constraint(c).unwrap(), want, "{c}");
        }
        for bad in ["dev-main", "^9.0", "", "8.0 - 8.2", ">=8.1@dev", "^8.x"] {
            let e = php_from_constraint(bad).expect_err(bad);
            assert_eq!(e.hint.as_deref(), Some("pass --php <version>"), "{bad}");
        }
    }
}
