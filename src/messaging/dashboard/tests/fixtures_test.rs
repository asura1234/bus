//! Expand synthetic fixture overlays without repeating inert persisted fields.
use serde_json::Value;
pub(super) fn cases() -> Vec<Value> {
    let document: Value = serde_json::from_str(include_str!("fixtures/requests.json")).unwrap();
    document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let mut case = case.clone();
            case["record"] = overlay(&document["base"], &case["record"]);
            case
        })
        .collect()
}
fn overlay(base: &Value, patch: &Value) -> Value {
    if let (Some(base), Some(patch)) = (base.as_object(), patch.as_object()) {
        let mut out = base.clone();
        for (key, value) in patch {
            let value = out
                .get(key)
                .map_or_else(|| value.clone(), |base| overlay(base, value));
            out.insert(key.clone(), value);
        }
        Value::Object(out)
    } else {
        patch.clone()
    }
}
