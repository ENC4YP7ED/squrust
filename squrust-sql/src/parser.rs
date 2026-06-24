//! Thin wrapper over `sqlparser`.

use sqlparser::ast::Statement;
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::error::{Result, SqlError};

/// Parse SQL text into a list of statements.
pub fn parse(sql: &str) -> Result<Vec<Statement>> {
    Parser::parse_sql(&SQLiteDialect {}, sql).map_err(|e| SqlError::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_select() {
        let stmts = parse("SELECT a, b FROM t WHERE a > 1").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn multiple_statements() {
        let stmts = parse("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);").unwrap();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn parse_error() {
        assert!(parse("SELECT FROM WHERE").is_err());
    }
}
