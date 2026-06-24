//! [`FromValue`], [`RowAccess`] and [`FromRow`] for mapping query results into
//! Rust types.

use squrust_sql::{SqlError, Value};

type Result<T> = std::result::Result<T, SqlError>;

/// Convert a single [`Value`] into a typed Rust value.
pub trait FromValue: Sized {
    fn from_value(value: &Value) -> Result<Self>;
}

fn conv_err(want: &str, v: &Value) -> SqlError {
    SqlError::Type(format!("cannot convert {:?} to {want}", v.sql_type()))
}

impl FromValue for i64 {
    fn from_value(v: &Value) -> Result<Self> {
        v.as_i64().ok_or_else(|| conv_err("i64", v))
    }
}
impl FromValue for i32 {
    fn from_value(v: &Value) -> Result<Self> {
        v.as_i64()
            .map(|i| i as i32)
            .ok_or_else(|| conv_err("i32", v))
    }
}
impl FromValue for f64 {
    fn from_value(v: &Value) -> Result<Self> {
        v.as_f64().ok_or_else(|| conv_err("f64", v))
    }
}
impl FromValue for f32 {
    fn from_value(v: &Value) -> Result<Self> {
        v.as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| conv_err("f32", v))
    }
}
impl FromValue for bool {
    fn from_value(v: &Value) -> Result<Self> {
        match v {
            Value::Null => Err(conv_err("bool", v)),
            other => Ok(other.is_truthy()),
        }
    }
}
impl FromValue for String {
    fn from_value(v: &Value) -> Result<Self> {
        match v {
            Value::Null => Err(conv_err("String", v)),
            other => Ok(other.to_display_string()),
        }
    }
}
impl FromValue for Vec<u8> {
    fn from_value(v: &Value) -> Result<Self> {
        match v {
            Value::Blob(b) => Ok(b.clone()),
            Value::Text(t) => Ok(t.clone().into_bytes()),
            other => Err(conv_err("Vec<u8>", other)),
        }
    }
}
impl FromValue for serde_json::Value {
    fn from_value(v: &Value) -> Result<Self> {
        match v {
            Value::Json(j) => Ok(j.clone()),
            Value::Text(t) => serde_json::from_str(t)
                .map_err(|e| SqlError::Type(format!("invalid JSON: {e}"))),
            other => Ok(serde_json::Value::String(other.to_display_string())),
        }
    }
}
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: &Value) -> Result<Self> {
        if v.is_null() {
            Ok(None)
        } else {
            T::from_value(v).map(Some)
        }
    }
}

/// Column access for a result row. Implemented by the async layer's row type.
pub trait RowAccess {
    fn value(&self, idx: usize) -> Option<&Value>;
    fn value_by_name(&self, name: &str) -> Option<&Value>;
    fn ncols(&self) -> usize;

    /// Typed column access by position.
    fn get<T: FromValue>(&self, idx: usize) -> Result<T> {
        let v = self
            .value(idx)
            .ok_or_else(|| SqlError::Type(format!("no column at index {idx}")))?;
        T::from_value(v)
    }

    /// Typed column access by name.
    fn get_by_name<T: FromValue>(&self, name: &str) -> Result<T> {
        let v = self
            .value_by_name(name)
            .ok_or_else(|| SqlError::NotFound(format!("column `{name}`")))?;
        T::from_value(v)
    }
}

/// Build a value of `Self` from a result row. Implemented for primitives,
/// `Option`, tuples, and (via the derive macro) user structs.
pub trait FromRow: Sized {
    fn from_row<R: RowAccess + ?Sized>(row: &R) -> Result<Self>;
}

// Single-column rows map column 0 to the value.
macro_rules! from_row_scalar {
    ($($t:ty),*) => {$(
        impl FromRow for $t {
            fn from_row<R: RowAccess + ?Sized>(row: &R) -> Result<Self> {
                row.get::<$t>(0)
            }
        }
    )*};
}
from_row_scalar!(i64, i32, f64, f32, bool, String, Vec<u8>, serde_json::Value);

impl<T: FromValue> FromRow for Option<T> {
    fn from_row<R: RowAccess + ?Sized>(row: &R) -> Result<Self> {
        match row.value(0) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => T::from_value(v).map(Some),
        }
    }
}

// Tuple rows: each element maps to successive columns.
macro_rules! from_row_tuple {
    ($($idx:tt : $name:ident),+) => {
        impl<$($name: FromValue),+> FromRow for ($($name,)+) {
            fn from_row<R: RowAccess + ?Sized>(row: &R) -> Result<Self> {
                Ok(( $( row.get::<$name>($idx)?, )+ ))
            }
        }
    };
}
from_row_tuple!(0: A);
from_row_tuple!(0: A, 1: B);
from_row_tuple!(0: A, 1: B, 2: C);
from_row_tuple!(0: A, 1: B, 2: C, 3: D);
from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E);
from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRow {
        names: Vec<String>,
        values: Vec<Value>,
    }

    impl RowAccess for TestRow {
        fn value(&self, idx: usize) -> Option<&Value> {
            self.values.get(idx)
        }
        fn value_by_name(&self, name: &str) -> Option<&Value> {
            self.names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(name))
                .and_then(|i| self.values.get(i))
        }
        fn ncols(&self) -> usize {
            self.values.len()
        }
    }

    fn row() -> TestRow {
        TestRow {
            names: vec!["id".into(), "name".into(), "score".into()],
            values: vec![
                Value::Integer(7),
                Value::Text("ada".into()),
                Value::Null,
            ],
        }
    }

    #[test]
    fn scalar_and_tuple() {
        let r = row();
        let id: i64 = i64::from_row(&r).unwrap();
        assert_eq!(id, 7);

        let t: (i64, String, Option<f64>) = FromRow::from_row(&r).unwrap();
        assert_eq!(t.0, 7);
        assert_eq!(t.1, "ada");
        assert_eq!(t.2, None);
    }

    #[test]
    fn by_name_and_null() {
        let r = row();
        let name: String = r.get_by_name("name").unwrap();
        assert_eq!(name, "ada");
        let score: Option<f64> = r.get_by_name("score").unwrap();
        assert_eq!(score, None);
    }
}
