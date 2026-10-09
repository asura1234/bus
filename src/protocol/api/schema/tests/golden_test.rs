use super::*;

fn protocol_schema_entry<T: schemars::JsonSchema>(name: &str) -> serde_json::Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    rewrite_schema_refs(&mut schema, name);
    schema
}

fn rewrite_schema_refs(value: &mut serde_json::Value, schema_name: &str) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(serde_json::Value::String(reference)) = object.get_mut("$ref") {
                if let Some(path) = reference.strip_prefix("#/") {
                    *reference = format!("#/schemas/{schema_name}/{path}");
                }
            }
            for child in object.values_mut() {
                rewrite_schema_refs(child, schema_name);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_schema_refs(item, schema_name);
            }
        }
        _ => {}
    }
}

fn protocol_schema_document() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Herdr API",
        "schema_version": 1,
        "protocol": crate::protocol::wire::PROTOCOL_VERSION,
        "schemas": {
            "request": protocol_schema_entry::<Request>("request"),
            "success_response": protocol_schema_entry::<SuccessResponse>("success_response"),
            "error_response": protocol_schema_entry::<ErrorResponse>("error_response"),
            "event": protocol_schema_entry::<EventEnvelope>("event"),
            "subscription_event": protocol_schema_entry::<SubscriptionEventEnvelope>("subscription_event"),
        },
    })
}

#[test]
fn bundled_protocol_schema_refs_resolve_inside_bundle() {
    fn assert_no_standalone_refs(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(serde_json::Value::String(reference)) = object.get("$ref") {
                    assert!(
                        !reference.starts_with("#/$defs/"),
                        "schema bundle contains standalone ref {reference}"
                    );
                }
                for child in object.values() {
                    assert_no_standalone_refs(child);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    assert_no_standalone_refs(item);
                }
            }
            _ => {}
        }
    }

    assert_no_standalone_refs(&protocol_schema_document());
}

#[test]
fn generated_protocol_schema_artifact_is_current() {
    let actual = format!(
        "{}\n",
        serde_json::to_string_pretty(&protocol_schema_document()).unwrap()
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/protocol/api/schema/herdr-api.schema.json");

    if std::env::var_os("HERDR_UPDATE_API_SCHEMA").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "failed to read {}; run `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current`: {err}",
            path.display()
        )
    });
    assert_eq!(
        expected,
        actual,
        "generated API schema artifact is stale; run `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current`"
    );
}
