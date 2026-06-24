//! SQL value and type model, with SQLite-style affinity and comparison rules.

use std::cmp::Ordering;

use crate::error::{Result, SqlError};

/// Declared/affinity types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlType {
    Null,
    Integer,
    Real,
    Text,
    Blob,
    Boolean,
    Json,
}

impl SqlType {
    /// Map a declared column type string to an affinity, following SQLite's
    /// substring rules (<https://www.sqlite.org/datatype3.html#affinity>).
    pub fn affinity_from_decl(decl: &str) -> SqlType {
        let d = decl.to_ascii_uppercase();
        if d.contains("INT") {
            SqlType::Integer
        } else if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") {
            SqlType::Text
        } else if d.contains("BLOB") || d.is_empty() {
            SqlType::Blob
        } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
            SqlType::Real
        } else if d.contains("BOOL") {
            SqlType::Boolean
        } else if d.contains("JSON") {
            SqlType::Json
        } else {
            // NUMERIC affinity — we treat as Integer-preferring numeric.
            SqlType::Integer
        }
    }
}

/// A dynamically-typed SQL value.
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    Boolean(bool),
    Json(serde_json::Value),
}

impl Value {
    pub fn sql_type(&self) -> SqlType {
        match self {
            Value::Null => SqlType::Null,
            Value::Integer(_) => SqlType::Integer,
            Value::Real(_) => SqlType::Real,
            Value::Text(_) => SqlType::Text,
            Value::Blob(_) => SqlType::Blob,
            Value::Boolean(_) => SqlType::Boolean,
            Value::Json(_) => SqlType::Json,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Truthiness for WHERE/boolean contexts (SQLite: non-zero numeric is true).
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Boolean(b) => *b,
            Value::Integer(i) => *i != 0,
            Value::Real(r) => *r != 0.0,
            Value::Text(t) => !t.is_empty() && t.parse::<f64>().map(|n| n != 0.0).unwrap_or(false),
            Value::Blob(b) => !b.is_empty(),
            Value::Json(_) => true,
        }
    }

    /// Numeric view of the value, if it can be interpreted as a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            Value::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Text(t) => t.parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            Value::Real(r) => Some(*r as i64),
            Value::Boolean(b) => Some(if *b { 1 } else { 0 }),
            Value::Text(t) => t.parse::<i64>().ok(),
            _ => None,
        }
    }

    /// Apply SQLite column affinity when storing a value into a column.
    /// See <https://www.sqlite.org/datatype3.html#affinity>.
    pub fn coerce_to(&self, target: SqlType) -> Result<Value> {
        if self.is_null() {
            return Ok(Value::Null);
        }
        Ok(match target {
            // BLOB (a.k.a. NONE) affinity performs no conversion at all.
            SqlType::Blob | SqlType::Null => self.clone(),
            // INTEGER / NUMERIC affinity: prefer integers, keeping reals with a
            // fractional part and non-numeric text unchanged.
            SqlType::Integer => apply_numeric(self, true),
            SqlType::Real => apply_numeric(self, false),
            SqlType::Boolean => Value::Boolean(self.is_truthy()),
            // TEXT affinity converts numeric storage to text; blobs are kept.
            SqlType::Text => match self {
                Value::Text(_) | Value::Blob(_) => self.clone(),
                other => Value::Text(other.to_display_string()),
            },
            SqlType::Json => match self {
                Value::Json(_) => self.clone(),
                Value::Text(t) => serde_json::from_str(t)
                    .map(Value::Json)
                    .map_err(|e| SqlError::Type(format!("invalid JSON: {e}")))?,
                other => Value::Json(serde_json::Value::String(other.to_display_string())),
            },
        })
    }

    /// String rendering used for display and TEXT coercion.
    pub fn to_display_string(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Integer(i) => i.to_string(),
            Value::Real(r) => format_real(*r),
            Value::Text(t) => t.clone(),
            Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Blob(b) => String::from_utf8_lossy(b).into_owned(),
            Value::Json(j) => j.to_string(),
        }
    }
}

/// Render a float as SQLite does: `%g`-style notation (`%!.17g`) — exponential
/// when the decimal exponent is `< -4` or `> 16`, otherwise fixed-point, always
/// keeping a decimal point so reals stay distinct from integers.
///
/// The significant digits come from Rust's shortest round-trip formatting, which
/// matches SQLite for integers and ordinary decimals; values that need the full
/// 17 digits (e.g. `1.0/3.0`) may differ in the final digit from SQLite's custom
/// decimal conversion. The stored f64 is, of course, identical.
fn format_real(r: f64) -> String {
    if r == 0.0 {
        return "0.0".to_string(); // also normalizes -0.0, like SQLite
    }
    if !r.is_finite() {
        return if r.is_nan() {
            String::new()
        } else if r < 0.0 {
            "-Inf".to_string()
        } else {
            "Inf".to_string()
        };
    }
    let neg = r < 0.0;
    // `{:e}` yields a normalized `d.dddde<exp>`: shortest round-trip digits and
    // the base-10 exponent of the leading digit.
    let s = format!("{:e}", r.abs());
    let (mant, exp_str) = s.split_once('e').unwrap();
    let exp: i32 = exp_str.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();

    let body = if !(-4..=16).contains(&exp) {
        format_real_exp(&digits, exp)
    } else {
        format_real_fixed(&digits, exp)
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

fn format_real_exp(digits: &str, exp: i32) -> String {
    let (first, rest) = digits.split_at(1);
    let frac = if rest.is_empty() { "0" } else { rest };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{first}.{frac}e{sign}{:02}", exp.abs())
}

fn format_real_fixed(digits: &str, exp: i32) -> String {
    let ndigits = digits.len() as i32;
    if exp >= 0 {
        let int_len = exp + 1;
        if ndigits <= int_len {
            let mut s = String::with_capacity(int_len as usize + 2);
            s.push_str(digits);
            for _ in 0..(int_len - ndigits) {
                s.push('0');
            }
            s.push_str(".0");
            s
        } else {
            let (i, f) = digits.split_at(int_len as usize);
            format!("{i}.{f}")
        }
    } else {
        format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
    }
}

/// Apply INTEGER/NUMERIC (`integer_pref = true`) or REAL (`false`) affinity.
fn apply_numeric(v: &Value, integer_pref: bool) -> Value {
    let as_int_if = |i: i64| {
        if integer_pref {
            Value::Integer(i)
        } else {
            Value::Real(i as f64)
        }
    };
    let real_to_value = |r: f64| {
        if integer_pref && r.is_finite() && r.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&r) {
            Value::Integer(r as i64)
        } else {
            Value::Real(r)
        }
    };
    match v {
        Value::Integer(i) => as_int_if(*i),
        Value::Boolean(b) => as_int_if(*b as i64),
        Value::Real(r) => real_to_value(*r),
        Value::Text(t) => {
            let s = t.trim();
            if let Ok(i) = s.parse::<i64>() {
                as_int_if(i)
            } else if let Ok(r) = s.parse::<f64>() {
                real_to_value(r)
            } else {
                Value::Text(t.clone())
            }
        }
        other => other.clone(),
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.compare(other) == Some(Ordering::Equal)
    }
}

impl Value {
    /// SQLite-style comparison. NULL compared to anything yields `None` (used
    /// for three-valued predicate logic). Otherwise NULL sorts lowest, then
    /// numbers, then text, then blobs.
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Null, Value::Null) => Some(Ordering::Equal),
            (Value::Null, _) | (_, Value::Null) => None,
            // numeric vs numeric (and booleans treated numerically)
            _ => {
                if let (Some(a), Some(b)) = (self.numeric_only(), other.numeric_only()) {
                    return a.partial_cmp(&b);
                }
                match (self, other) {
                    (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
                    (Value::Blob(a), Value::Blob(b)) => Some(a.cmp(b)),
                    // Mixed: order by storage class rank (numeric < text < blob).
                    _ => self.class_rank().partial_cmp(&other.class_rank()),
                }
            }
        }
    }

    /// Total ordering for ORDER BY (NULLs first, then the natural order).
    pub fn order_key(&self, other: &Value) -> Ordering {
        match (self, other) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            _ => self.compare(other).unwrap_or(Ordering::Equal),
        }
    }

    fn numeric_only(&self) -> Option<f64> {
        match self {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            Value::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn class_rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Integer(_) | Value::Real(_) | Value::Boolean(_) => 1,
            Value::Text(_) | Value::Json(_) => 2,
            Value::Blob(_) => 3,
        }
    }
}

// Conversions into Value (used by parameter binding and serde).
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Integer(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Integer(v as i64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Real(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Real(v as f64)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Boolean(v)
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Blob(v)
    }
}
impl From<serde_json::Value> for Value {
    fn from(v: serde_json::Value) -> Self {
        Value::Json(v)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(x) => x.into(),
            None => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affinity_rules() {
        assert_eq!(SqlType::affinity_from_decl("INTEGER"), SqlType::Integer);
        assert_eq!(SqlType::affinity_from_decl("VARCHAR(20)"), SqlType::Text);
        assert_eq!(SqlType::affinity_from_decl("REAL"), SqlType::Real);
        assert_eq!(SqlType::affinity_from_decl("BLOB"), SqlType::Blob);
        assert_eq!(SqlType::affinity_from_decl(""), SqlType::Blob);
    }

    #[test]
    fn numeric_comparison() {
        assert_eq!(
            Value::Integer(1).compare(&Value::Real(1.0)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            Value::Integer(1).compare(&Value::Integer(2)),
            Some(Ordering::Less)
        );
        assert_eq!(Value::Null.compare(&Value::Integer(1)), None);
    }

    #[test]
    fn order_puts_nulls_first() {
        let mut v = [Value::Integer(3), Value::Null, Value::Integer(1)];
        v.sort_by(|a, b| a.order_key(b));
        assert!(matches!(v[0], Value::Null));
        assert!(matches!(v[1], Value::Integer(1)));
    }

    #[test]
    fn coercion() {
        assert!(matches!(
            Value::Text("42".into()).coerce_to(SqlType::Integer).unwrap(),
            Value::Integer(42)
        ));
        assert!(matches!(
            Value::Integer(1).coerce_to(SqlType::Text).unwrap(),
            Value::Text(_)
        ));
    }
}

#[cfg(test)]
mod float_fmt_tests {
    use super::format_real;

    #[test]
    fn matches_sqlite_notation() {
        // Fixed-point and integer-valued reals keep a decimal point.
        assert_eq!(format_real(1.0), "1.0");
        assert_eq!(format_real(100.0), "100.0");
        assert_eq!(format_real(1.5), "1.5");
        assert_eq!(format_real(0.1), "0.1");
        assert_eq!(format_real(0.0001), "0.0001");
        assert_eq!(format_real(123.456), "123.456");
        assert_eq!(format_real(123456789012345.0), "123456789012345.0");
        // -0.0 normalizes to "0.0", as SQLite does.
        assert_eq!(format_real(-0.0), "0.0");
        assert_eq!(format_real(0.0), "0.0");
        assert_eq!(format_real(-2.5), "-2.5");
        // Exponential when exp < -4 or > 16.
        assert_eq!(format_real(1e20), "1.0e+20");
        assert_eq!(format_real(1e-10), "1.0e-10");
        assert_eq!(format_real(1e17), "1.0e+17");
        assert_eq!(format_real(1e16), "10000000000000000.0");
        assert_eq!(format_real(1e-4), "0.0001");
        assert_eq!(format_real(1e-5), "1.0e-05");
        assert_eq!(format_real(0.000012345), "1.2345e-05");
    }
}
