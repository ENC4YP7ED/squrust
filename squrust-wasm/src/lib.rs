//! # squrust-wasm
//!
//! Browser-facing wasm bindings for Squrust.
//!
//! The portable database logic lives in [`Db`], which works on any target and
//! is covered by host tests. The `wasm-bindgen`-exported [`SqurustDb`] class
//! (gated to `wasm32`) is a thin wrapper that turns `Db`'s async methods into
//! JavaScript Promises and JSON values.
//!
//! ## Status
//!
//! Persistence currently uses an in-memory engine. A true OPFS-backed build
//! additionally requires a wasm-compatible storage backend in `squrust-core`
//! (its file I/O is Unix-specific today); that backend is the remaining piece
//! before `wasm-pack build` produces a persistent browser database.

#![forbid(unsafe_code)]

use squrust_async::{SqurustConnection, SqurustError, Value};

/// A portable database handle. JSON in, JSON out.
pub struct Db {
    conn: SqurustConnection,
}

impl Db {
    /// Open an in-memory database.
    pub async fn open_memory() -> Result<Db, SqurustError> {
        Ok(Db {
            conn: SqurustConnection::open_memory().await?,
        })
    }

    /// Run a SELECT and return rows as JSON objects keyed by column name.
    pub async fn query_json(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<Vec<serde_json::Value>, SqurustError> {
        let (cols, rows) = self.conn.fetch_raw(sql, params).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut obj = serde_json::Map::new();
            for (i, v) in row.into_iter().enumerate() {
                let key = cols.get(i).cloned().unwrap_or_else(|| i.to_string());
                obj.insert(key, value_to_json(v));
            }
            out.push(serde_json::Value::Object(obj));
        }
        Ok(out)
    }

    /// Run a statement (DDL/DML) and return rows affected.
    pub async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<u64, SqurustError> {
        self.conn.execute(sql, params).await
    }
}

/// Convert a JSON parameter into a Squrust [`Value`].
pub fn json_to_value(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        other => Value::Json(other.clone()),
    }
}

fn value_to_json(v: Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Integer(i) => J::from(i),
        Value::Real(r) => serde_json::Number::from_f64(r).map(J::Number).unwrap_or(J::Null),
        Value::Boolean(b) => J::Bool(b),
        Value::Text(t) => J::String(t),
        Value::Json(j) => j,
        Value::Blob(b) => J::String(String::from_utf8_lossy(&b).into_owned()),
    }
}

// ---------------------------------------------------------------------------
// wasm-bindgen surface (browser only)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{Db, json_to_value};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::future_to_promise;

    /// ```typescript
    /// class SqurustDb {
    ///   static openMemory(): Promise<SqurustDb>;
    ///   query(sql: string, params?: unknown[]): Promise<Record<string, unknown>[]>;
    ///   execute(sql: string, params?: unknown[]): Promise<number>;
    ///   close(): Promise<void>;
    /// }
    /// ```
    #[wasm_bindgen]
    pub struct SqurustDb {
        inner: Rc<RefCell<Db>>,
    }

    #[wasm_bindgen]
    impl SqurustDb {
        #[wasm_bindgen(js_name = openMemory)]
        pub async fn open_memory() -> Result<SqurustDb, JsValue> {
            let db = Db::open_memory().await.map_err(err)?;
            Ok(SqurustDb {
                inner: Rc::new(RefCell::new(db)),
            })
        }

        pub fn query(&self, sql: String, params: JsValue) -> js_sys::Promise {
            let inner = self.inner.clone();
            future_to_promise(async move {
                let params = parse_params(params)?;
                let rows = inner.borrow().query_json(&sql, params).await.map_err(err)?;
                serde_wasm_bindgen::to_value(&rows).map_err(|e| JsValue::from_str(&e.to_string()))
            })
        }

        pub fn execute(&self, sql: String, params: JsValue) -> js_sys::Promise {
            let inner = self.inner.clone();
            future_to_promise(async move {
                let params = parse_params(params)?;
                let n = inner.borrow().execute(&sql, params).await.map_err(err)?;
                Ok(JsValue::from_f64(n as f64))
            })
        }

        pub fn close(&self) -> js_sys::Promise {
            future_to_promise(async move { Ok(JsValue::UNDEFINED) })
        }
    }

    fn parse_params(params: JsValue) -> Result<Vec<squrust_async::Value>, JsValue> {
        if params.is_undefined() || params.is_null() {
            return Ok(vec![]);
        }
        let json: serde_json::Value = serde_wasm_bindgen::from_value(params)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        match json {
            serde_json::Value::Array(items) => Ok(items.iter().map(json_to_value).collect()),
            single => Ok(vec![json_to_value(&single)]),
        }
    }

    fn err(e: impl std::fmt::Display) -> JsValue {
        JsValue::from_str(&e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn portable_db_roundtrip() {
        let db = Db::open_memory().await.unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)", vec![])
            .await
            .unwrap();
        let n = db
            .execute(
                "INSERT INTO t(id, name) VALUES (?, ?)",
                vec![Value::Integer(1), Value::Text("ada".into())],
            )
            .await
            .unwrap();
        assert_eq!(n, 1);

        let rows = db
            .query_json("SELECT id, name FROM t", vec![])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], serde_json::json!(1));
        assert_eq!(rows[0]["name"], serde_json::json!("ada"));
    }

    #[test]
    fn json_param_conversion() {
        assert!(matches!(
            json_to_value(&serde_json::json!(5)),
            Value::Integer(5)
        ));
        assert!(matches!(
            json_to_value(&serde_json::json!("x")),
            Value::Text(_)
        ));
        assert!(matches!(json_to_value(&serde_json::json!(null)), Value::Null));
    }
}
