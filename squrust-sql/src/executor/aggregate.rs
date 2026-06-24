//! Aggregation with optional GROUP BY.

use std::cmp::Ordering;

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::eval::eval;
use crate::executor::{Executor, Params};
use crate::planner::{AggExpr, AggFunc, ColumnInfo, Expr, OutputCol};
use crate::row::Row;
use crate::types::Value;

pub struct AggExec {
    input: Option<Box<dyn Executor>>,
    group_by: Vec<Expr>,
    aggs: Vec<AggExpr>,
    output: Vec<OutputCol>,
    columns: Vec<ColumnInfo>,
    base_len: usize,
    having: Option<Expr>,
    params: Params,
    produced: Option<std::vec::IntoIter<Row>>,
}

impl AggExec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        input: Box<dyn Executor>,
        group_by: Vec<Expr>,
        aggs: Vec<AggExpr>,
        output: Vec<OutputCol>,
        columns: Vec<ColumnInfo>,
        base_len: usize,
        having: Option<Expr>,
        params: Params,
    ) -> Self {
        AggExec {
            input: Some(input),
            group_by,
            aggs,
            output,
            columns,
            base_len,
            having,
            params,
            produced: None,
        }
    }

    async fn compute(&mut self) -> Result<Vec<Row>> {
        let mut input = self.input.take().expect("compute called once");
        let rows = input.collect_all().await?;

        let mut groups: Vec<Group> = Vec::new();

        for row in &rows {
            let key = self
                .group_by
                .iter()
                .map(|e| eval(e, &row.values, row.row_id, &self.params))
                .collect::<Result<Vec<_>>>()?;
            let idx = match groups.iter().position(|g| keys_eq(&g.key, &key)) {
                Some(i) => i,
                None => {
                    groups.push(Group::new(key, self.aggs.len(), row.clone()));
                    groups.len() - 1
                }
            };
            let group = &mut groups[idx];
            for (ai, agg) in self.aggs.iter().enumerate() {
                let v = match &agg.arg {
                    Some(arg) => eval(arg, &row.values, row.row_id, &self.params)?,
                    None => Value::Null,
                };
                group.accs[ai].update(agg, v);
            }
        }

        // No GROUP BY over an empty input still yields one (all-NULL/zero) row.
        if self.group_by.is_empty() && groups.is_empty() {
            groups.push(Group::new(vec![], self.aggs.len(), Row::default()));
        } else {
            // Deterministic ordering by group key.
            groups.sort_by(|a, b| cmp_keys(&a.key, &b.key));
        }

        let mut out = Vec::with_capacity(groups.len());
        for group in groups {
            // Finalize every aggregate once; reused by HAVING and the output.
            let finalized: Vec<Value> = self
                .aggs
                .iter()
                .enumerate()
                .map(|(i, a)| group.accs[i].finalize(a))
                .collect();

            // HAVING filters groups, evaluated over [input cols .. | agg results].
            if let Some(h) = &self.having {
                let mut augmented = group.rep.values.clone();
                augmented.resize(self.base_len, Value::Null);
                augmented.extend(finalized.iter().cloned());
                if !eval(h, &augmented, group.rep.row_id, &self.params)?.is_truthy() {
                    continue;
                }
            }

            let mut values = Vec::with_capacity(self.output.len());
            for col in &self.output {
                match col {
                    OutputCol::Agg(i) => values.push(finalized[*i].clone()),
                    OutputCol::Expr(e) => {
                        let v = if group.has_rep {
                            eval(e, &group.rep.values, group.rep.row_id, &self.params)?
                        } else {
                            Value::Null
                        };
                        values.push(v);
                    }
                }
            }
            out.push(Row::new(0, values));
        }
        Ok(out)
    }
}

#[async_trait]
impl Executor for AggExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        if self.produced.is_none() {
            let rows = self.compute().await?;
            self.produced = Some(rows.into_iter());
        }
        Ok(self.produced.as_mut().unwrap().next())
    }
}

struct Group {
    key: Vec<Value>,
    accs: Vec<Acc>,
    rep: Row,
    has_rep: bool,
}

impl Group {
    fn new(key: Vec<Value>, n_aggs: usize, rep: Row) -> Self {
        let has_rep = !rep.values.is_empty() || rep.row_id != 0;
        Group {
            key,
            accs: (0..n_aggs).map(|_| Acc::default()).collect(),
            rep,
            has_rep,
        }
    }
}

#[derive(Default)]
struct Acc {
    count: i64,
    sum: f64,
    sum_seen: bool,
    extreme: Option<Value>,
    distinct: Vec<Value>,
}

impl Acc {
    fn update(&mut self, agg: &AggExpr, v: Value) {
        match agg.func {
            AggFunc::CountStar => self.count += 1,
            _ => {
                if v.is_null() {
                    return;
                }
                if agg.distinct {
                    if self.distinct.iter().any(|d| d == &v) {
                        return;
                    }
                    self.distinct.push(v.clone());
                }
                match agg.func {
                    AggFunc::Count => self.count += 1,
                    AggFunc::Sum | AggFunc::Avg => {
                        if let Some(n) = v.as_f64() {
                            self.sum += n;
                            self.sum_seen = true;
                            self.count += 1;
                        }
                    }
                    AggFunc::Min => {
                        if self.extreme.as_ref().map_or(true, |e| {
                            v.compare(e) == Some(Ordering::Less)
                        }) {
                            self.extreme = Some(v);
                        }
                    }
                    AggFunc::Max => {
                        if self.extreme.as_ref().map_or(true, |e| {
                            v.compare(e) == Some(Ordering::Greater)
                        }) {
                            self.extreme = Some(v);
                        }
                    }
                    AggFunc::CountStar => unreachable!(),
                }
            }
        }
    }

    fn finalize(&self, agg: &AggExpr) -> Value {
        match agg.func {
            AggFunc::Count | AggFunc::CountStar => Value::Integer(self.count),
            AggFunc::Sum => {
                if self.sum_seen {
                    if self.sum.fract() == 0.0 {
                        Value::Integer(self.sum as i64)
                    } else {
                        Value::Real(self.sum)
                    }
                } else {
                    Value::Null
                }
            }
            AggFunc::Avg => {
                if self.count > 0 {
                    Value::Real(self.sum / self.count as f64)
                } else {
                    Value::Null
                }
            }
            AggFunc::Min | AggFunc::Max => self.extreme.clone().unwrap_or(Value::Null),
        }
    }
}

fn keys_eq(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y)
}

fn cmp_keys(a: &[Value], b: &[Value]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = x.order_key(y);
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}
