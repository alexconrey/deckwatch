use super::*;
use crate::handlers::mcp_telemetry::DECKWATCH_TOOLS;
use serde_json::json;

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "replicas": { "type": "integer", "minimum": 0 },
            "kind": { "type": "string", "enum": ["a", "b"] },
            "tags": { "type": "array", "items": { "type": "string" } },
            "labels": { "type": "object", "additionalProperties": { "type": "string" } },
            "config": {
                "type": "object",
                "additionalProperties": {
                    "anyOf": [{ "type": "string" }, { "type": "null" }]
                }
            },
            "source": {
                "type": "object",
                "properties": { "type": { "type": "string" } },
                "required": ["type"],
                "additionalProperties": false
            }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn err(args: Value) -> String {
    validate(&schema(), &args).unwrap_err().to_string()
}

#[test]
fn accepts_what_the_schema_declares() {
    let args = json!({
        "name": "web",
        "replicas": 0,
        "kind": "b",
        "tags": ["x"],
        "labels": { "app": "web" },
        "config": { "KEEP": "1", "DROP": null },
        "source": { "type": "url" },
    });
    assert_eq!(validate(&schema(), &args), Ok(()));
}

#[test]
fn refuses_unknown_arguments_first_and_names_them() {
    assert_eq!(
        err(json!({ "name": "web", "extra": 1 })),
        "unknown argument `extra`"
    );
    // Before a missing required argument or a bad value.
    assert_eq!(
        err(json!({ "replicas": "x", "extra": 1 })),
        "unknown argument `extra`"
    );
    assert_eq!(
        err(json!({ "name": "web", "source": { "type": "url", "filename": "x" } })),
        "unknown argument `source.filename`"
    );
}

#[test]
fn refuses_missing_required_arguments() {
    assert_eq!(err(json!({})), "missing required argument `name`");
    assert_eq!(
        err(json!({ "name": null })),
        "missing required argument `name`"
    );
    assert_eq!(
        err(json!({ "name": "web", "source": {} })),
        "missing required argument `source.type`"
    );
    assert_eq!(
        validate(&schema(), &Value::Null).unwrap_err(),
        Invalid::Missing("name".into()),
        "absent arguments are an empty object"
    );
}

#[test]
fn refuses_values_of_the_wrong_shape() {
    assert_eq!(err(json!({ "name": 3 })), "`name` must be a string");
    assert_eq!(
        err(json!({ "name": "web", "replicas": "3" })),
        "`replicas` must be an integer"
    );
    assert_eq!(
        err(json!({ "name": "web", "replicas": 1.5 })),
        "`replicas` must be an integer"
    );
    assert_eq!(
        err(json!({ "name": "web", "replicas": -1 })),
        "`replicas` must be at least 0"
    );
    assert_eq!(
        err(json!({ "name": "web", "kind": "c" })),
        r#"`kind` must be one of "a", "b""#
    );
    assert_eq!(
        err(json!({ "name": "web", "tags": ["x", 2] })),
        "`tags[1]` must be a string"
    );
    assert_eq!(
        err(json!({ "name": "web", "labels": { "a": 1 } })),
        "`labels.a` must be a string"
    );
    assert_eq!(
        err(json!({ "name": "web", "config": { "K": 1 } })),
        "`config.K` must be a string or null"
    );
    assert_eq!(
        validate(&schema(), &json!(["name"])).unwrap_err(),
        Invalid::NotAnObject
    );
}

#[test]
fn null_leaves_an_optional_argument_out() {
    assert_eq!(
        validate(
            &schema(),
            &json!({ "name": "web", "replicas": null, "kind": null })
        ),
        Ok(())
    );
}

/// The keywords [`validate`] enforces, plus `description`. A schema using any
/// other would have a constraint the check silently ignores.
const UNDERSTOOD: &[&str] = &[
    "type",
    "properties",
    "additionalProperties",
    "required",
    "enum",
    "items",
    "anyOf",
    "minimum",
    "description",
];

fn assert_understood(tool: &str, path: &str, schema: &Value) {
    let Some(obj) = schema.as_object() else {
        panic!("{tool}: {path} is not a schema object");
    };
    for (key, value) in obj {
        assert!(
            UNDERSTOOD.contains(&key.as_str()),
            "{tool}: {path} uses `{key}`, which the argument check doesn't enforce"
        );
        match key.as_str() {
            "properties" => {
                for (name, property) in value.as_object().expect("properties") {
                    assert_understood(tool, &format!("{path}.{name}"), property);
                }
            }
            "items" => assert_understood(tool, &format!("{path}[]"), value),
            "additionalProperties" if value.is_object() => {
                assert_understood(tool, &format!("{path}.*"), value)
            }
            "anyOf" => {
                for form in value.as_array().expect("anyOf") {
                    assert_understood(tool, path, form);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn every_native_schema_is_closed_and_fully_enforced() {
    let tools = crate::handlers::mcp::native_tool_definitions();
    assert!(tools.len() > 30, "{}", tools.len());
    for tool in &tools {
        let name = tool["name"].as_str().expect("name");
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{name}: its schema must refuse undeclared arguments"
        );
        assert_understood(name, name, schema);
        let e = check(name, &json!({ "not_an_argument": "x" })).unwrap_err();
        assert!(
            e.contains("unknown argument `not_an_argument`"),
            "{name}: {e}"
        );
    }
}

#[test]
fn every_deckwatch_tool_is_checked_or_deliberately_unchecked() {
    for tool in DECKWATCH_TOOLS {
        if UNCHECKED.contains(&tool.name) {
            continue;
        }
        assert!(
            schemas().contains_key(tool.name),
            "{} has no native definition, so calls of it are refused",
            tool.name
        );
    }
    for name in UNCHECKED {
        assert!(
            DECKWATCH_TOOLS.iter().any(|t| t.name == *name),
            "{name} is unchecked but not a deckwatch tool"
        );
        assert!(!schemas().contains_key(*name), "{name} has a native schema");
        assert_eq!(check(name, &json!({ "anything": 1 })), Ok(()), "{name}");
    }
    let e = check("no_such_tool", &json!({})).unwrap_err();
    assert!(e.contains("has no input schema"), "{e}");
}

#[test]
fn validate_plugin_names_why_it_refuses_an_argument() {
    for key in [
        "token_secret",
        "allowed_hosts",
        "inherit_env_keys",
        "inherit_env_file_keys",
    ] {
        let mut args = json!({ "source": { "type": "url", "url": "https://example.com/p.wasm" } });
        args[key] = json!(["X"]);
        let e = check("validate_plugin", &args).unwrap_err();
        assert!(e.contains(&format!("unknown argument `{key}`")), "{e}");
        assert!(e.contains("no git token, no network access"), "{e}");
    }
    let e = check(
        "validate_plugin",
        &json!({ "source": { "type": "upload", "filename": "x.wasm" } }),
    )
    .unwrap_err();
    assert!(e.contains("unknown argument `source.filename`"), "{e}");
    let e = check(
        "validate_plugin",
        &json!({ "source": { "type": "upload" } }),
    )
    .unwrap_err();
    assert!(
        e.contains(r#"`source.type` must be one of "github", "url""#),
        "{e}"
    );
}
