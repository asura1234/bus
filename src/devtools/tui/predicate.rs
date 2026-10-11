//! `wait --state 'PATH OP VALUE'` over the JSON of `bus state`.
//!
//! PATH is dotted (`visible_room`, `rooms.0.name`); `*` matches any element of
//! an array, and the predicate holds when any match satisfies it. OP is one of
//! `==`, `!=`, `contains`, `>`, `>=`, `<`, `<=`. VALUE is JSON, or a bare word
//! taken as a string.
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Predicate {
    pub source: String,
    path: Vec<String>,
    op: Op,
    value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Contains,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Predicate {
    pub fn parse(source: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "state predicate {source:?} must look like 'visible_room == 2' or 'rooms.*.name contains \"review\"'"
            )
        };
        let trimmed = source.trim();
        let (path, rest) = trimmed.split_once(char::is_whitespace).ok_or_else(bad)?;
        let rest = rest.trim_start();
        let (op, value) = rest.split_once(char::is_whitespace).ok_or_else(bad)?;
        let op = match op {
            "==" => Op::Eq,
            "!=" => Op::Ne,
            "contains" => Op::Contains,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "<" => Op::Lt,
            "<=" => Op::Le,
            _ => return Err(bad()),
        };
        let value = value.trim();
        let value = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
        Ok(Self {
            source: trimmed.to_owned(),
            path: path.split('.').map(str::to_owned).collect(),
            op,
            value,
        })
    }

    pub fn holds(&self, root: &Value) -> bool {
        let mut found = Vec::new();
        select(root, &self.path, &mut found);
        // `!=` over a missing path is true: nothing there equals the value.
        if found.is_empty() {
            return self.op == Op::Ne;
        }
        found.iter().any(|value| self.test(value))
    }

    /// What the path selected, for a timeout report.
    pub fn actual(&self, root: &Value) -> Value {
        let mut found = Vec::new();
        select(root, &self.path, &mut found);
        match found.as_slice() {
            [] => Value::Null,
            [one] => (*one).clone(),
            many => json!(many),
        }
    }

    fn test(&self, actual: &Value) -> bool {
        match self.op {
            Op::Eq => loosely_equal(actual, &self.value),
            Op::Ne => !loosely_equal(actual, &self.value),
            Op::Contains => match (actual, &self.value) {
                (Value::String(a), Value::String(b)) => a.contains(b.as_str()),
                (Value::Array(items), wanted) => items.iter().any(|i| loosely_equal(i, wanted)),
                _ => false,
            },
            Op::Gt | Op::Ge | Op::Lt | Op::Le => match (actual.as_f64(), self.value.as_f64()) {
                (Some(a), Some(b)) => match self.op {
                    Op::Gt => a > b,
                    Op::Ge => a >= b,
                    Op::Lt => a < b,
                    _ => a <= b,
                },
                _ => false,
            },
        }
    }
}

/// Numbers compare as numbers; a string compares with a number's text, so
/// `visible_room == 2` and `visible_room == "2"` both work.
fn loosely_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
            s == &n.to_string()
        }
        _ => a == b,
    }
}

fn select<'a>(value: &'a Value, path: &[String], out: &mut Vec<&'a Value>) {
    let Some((head, rest)) = path.split_first() else {
        out.push(value);
        return;
    };
    match value {
        Value::Array(items) if head == "*" => {
            for item in items {
                select(item, rest, out);
            }
        }
        Value::Object(map) if head == "*" => {
            for item in map.values() {
                select(item, rest, out);
            }
        }
        Value::Array(items) => {
            if let Some(item) = head.parse::<usize>().ok().and_then(|i| items.get(i)) {
                select(item, rest, out);
            }
        }
        Value::Object(map) => {
            if let Some(item) = map.get(head) {
                select(item, rest, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "tests/predicate_test.rs"]
mod tests;
