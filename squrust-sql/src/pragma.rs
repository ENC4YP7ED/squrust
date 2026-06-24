//! A small standalone parser for `PRAGMA` statements.
//!
//! SQLite accepts an unquoted identifier argument (e.g. `PRAGMA
//! table_info(users)`) that sqlparser's pragma grammar rejects, so Squrust
//! recognizes pragmas with this parser before handing SQL to sqlparser.

/// A parsed `PRAGMA` statement.
#[derive(Debug, Clone)]
pub struct Pragma {
    /// Lower-cased pragma name, with any `schema.` prefix stripped.
    pub name: String,
    /// The parenthesized argument, unquoted (e.g. the table for `table_info`).
    pub arg: Option<String>,
    /// The `= value`, unquoted (a "set" pragma).
    pub value: Option<String>,
}

/// Recognize a `PRAGMA` statement. Returns `None` if `sql` is not a pragma.
pub fn try_parse(sql: &str) -> Option<Pragma> {
    let s = sql.trim().trim_end_matches(';').trim();
    if s.len() < 7 || !s.as_bytes()[..6].eq_ignore_ascii_case(b"pragma") {
        return None;
    }
    if !s.as_bytes()[6].is_ascii_whitespace() {
        return None; // e.g. "pragmatic" is not a pragma
    }
    let rest = s[6..].trim();
    // A single statement only (no embedded statement separator).
    if rest.is_empty() || rest.contains(';') {
        return None;
    }

    let (head, arg, value) = if let Some(open) = rest.find('(') {
        let close = rest.rfind(')')?;
        if close < open {
            return None;
        }
        (&rest[..open], Some(unquote(rest[open + 1..close].trim())), None)
    } else if let Some(eq) = rest.find('=') {
        (&rest[..eq], None, Some(unquote(rest[eq + 1..].trim())))
    } else {
        (rest, None, None)
    };

    let name = head.trim().rsplit('.').next()?.trim().to_ascii_lowercase();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    Some(Pragma { name, arg, value })
}

fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'\'' && b[b.len() - 1] == b'\'')
            || (b[0] == b'"' && b[b.len() - 1] == b'"')
            || (b[0] == b'[' && b[b.len() - 1] == b']'))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forms() {
        let p = try_parse("PRAGMA table_info(users)").unwrap();
        assert_eq!(p.name, "table_info");
        assert_eq!(p.arg.as_deref(), Some("users"));

        let p = try_parse("pragma user_version = 42;").unwrap();
        assert_eq!(p.name, "user_version");
        assert_eq!(p.value.as_deref(), Some("42"));

        let p = try_parse("PRAGMA main.foreign_keys").unwrap();
        assert_eq!(p.name, "foreign_keys");
        assert!(p.arg.is_none() && p.value.is_none());

        assert!(try_parse("SELECT 1").is_none());
        assert!(try_parse("pragmatic things").is_none());
    }
}
