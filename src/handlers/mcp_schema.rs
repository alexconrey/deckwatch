//! Server-side checks of tool-call arguments against the `inputSchema` of
//! each of deckwatch's own MCP tools.
//!
//! `tools/list` tells a client which arguments a tool takes, but nothing used
//! to hold a call to that: a handler that read an argument its schema didn't
//! declare took it from any caller. [`check`] runs before every call of a
//! deckwatch-native tool and refuses, with an error naming the argument:
//!
//! - an argument the schema doesn't declare (every native schema sets
//!   `additionalProperties: false`, and a test keeps it that way);
//! - a missing required argument;
//! - a value of the wrong type, outside its `enum` or below its `minimum`.
//!
//! `null` passes for an optional argument: handlers read it as absent.
//!
//! The checker understands the keywords deckwatch's schemas use (`type`,
//! `properties`, `additionalProperties`, `required`, `enum`, `items`, `anyOf`,
//! `minimum`, `description`), and a test fails if a schema uses any other, so
//! no constraint in a schema is silently ignored.
//!
//! The mcp-k8s tools deckwatch replaces ([`UNCHECKED`]) aren't checked here:
//! they hand their arguments on to mcp-k8s, which accepts more than
//! deckwatch's versions of their schemas list.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// mcp-k8s tools deckwatch serves with its own handler but whose arguments
/// mcp-k8s also reads: `update_deployment` forwards fields deckwatch's schema
/// doesn't list, `exec_pod` takes `name` for `pod_name`, and so on.
pub const UNCHECKED: &[&str] = &["update_deployment", "apply_manifest", "exec_pod"];

/// Arguments a tool refuses on purpose, and why: the error says so. None of
/// them are in the tool's schema, so [`check`] is what refuses them; the
/// handler refuses them again.
const REFUSED: &[(&str, &[&str], &str)] = &[(
    "validate_plugin",
    &[
        "token_secret",
        "allowed_hosts",
        "inherit_env_keys",
        "inherit_env_file_keys",
    ],
    "a dry run gets no git token, no network access and nothing from \
     deckwatch's environment or files",
)];

/// Why `tool` refuses `argument`, if it does so on purpose.
pub fn refusal_reason(tool: &str, argument: &str) -> Option<&'static str> {
    REFUSED
        .iter()
        .find(|(t, args, _)| *t == tool && args.contains(&argument))
        .map(|(_, _, why)| *why)
}

/// Check the arguments of a call of deckwatch's own `tool` against its
/// schema. `Ok` for the tools in [`UNCHECKED`]; an error for a tool with no
/// schema, so a new tool can't skip the check by accident.
pub fn check(tool: &str, args: &Value) -> Result<(), String> {
    if UNCHECKED.contains(&tool) {
        return Ok(());
    }
    let schema = schemas()
        .get(tool)
        .ok_or_else(|| format!("{tool} has no input schema, so deckwatch refuses to run it"))?;
    validate(schema, args).map_err(|e| {
        let why = match &e {
            Invalid::Unknown(path) => refusal_reason(tool, path),
            _ => None,
        };
        match why {
            Some(why) => format!("invalid arguments for {tool}: {e}: {why}"),
            None => format!("invalid arguments for {tool}: {e}"),
        }
    })
}

/// Tool name → `inputSchema`, for every deckwatch-native tool.
fn schemas() -> &'static HashMap<String, Value> {
    static SCHEMAS: OnceLock<HashMap<String, Value>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        super::mcp::native_tool_definitions()
            .into_iter()
            .filter_map(|mut tool| {
                let name = tool["name"].as_str()?.to_string();
                Some((name, tool["inputSchema"].take()))
            })
            .collect()
    })
}

/// Why a call's arguments don't match the schema. Each names the argument
/// by its path (`source.type`, `config.KEY`, `allowed_hosts[0]`).
#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    /// The arguments aren't a JSON object.
    NotAnObject,
    /// An argument the schema doesn't declare.
    Unknown(String),
    /// A required argument is missing.
    Missing(String),
    /// A value of the wrong type, outside its `enum`, or below its `minimum`.
    Value { path: String, expected: String },
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnObject => write!(f, "the arguments must be a JSON object"),
            Self::Unknown(path) => write!(f, "unknown argument `{path}`"),
            Self::Missing(path) => write!(f, "missing required argument `{path}`"),
            Self::Value { path, expected } => write!(f, "`{path}` must be {expected}"),
        }
    }
}

/// Check `args`, a call's `arguments` (absent means `{}`), against a tool's
/// `inputSchema`. Unknown arguments are reported before anything else.
pub fn validate(schema: &Value, args: &Value) -> Result<(), Invalid> {
    match args {
        Value::Null => check_object(schema, &Map::new(), ""),
        Value::Object(obj) => check_object(schema, obj, ""),
        _ => Err(Invalid::NotAnObject),
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn check_object(schema: &Value, obj: &Map<String, Value>, path: &str) -> Result<(), Invalid> {
    let properties = schema.get("properties").and_then(Value::as_object);
    let additional = schema.get("additionalProperties");
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    // Unknown keys first: an argument the tool doesn't take is the most
    // useful thing to name.
    if let Some(Value::Bool(false)) = additional {
        if let Some(key) = obj
            .keys()
            .find(|k| !properties.is_some_and(|p| p.contains_key(k.as_str())))
        {
            return Err(Invalid::Unknown(join(path, key)));
        }
    }
    if let Some(key) = required
        .iter()
        .find(|k| obj.get(**k).is_none_or(Value::is_null))
    {
        return Err(Invalid::Missing(join(path, key)));
    }
    for (key, value) in obj {
        let child = join(path, key);
        match properties.and_then(|p| p.get(key)) {
            // An optional argument sent as null is absent.
            Some(_) if value.is_null() && !required.contains(&key.as_str()) => {}
            Some(property) => check_value(property, value, &child)?,
            None => {
                if let Some(extra @ Value::Object(_)) = additional {
                    check_value(extra, value, &child)?;
                }
            }
        }
    }
    Ok(())
}

fn check_value(schema: &Value, value: &Value, path: &str) -> Result<(), Invalid> {
    let invalid = |expected: String| Invalid::Value {
        path: path.to_string(),
        expected,
    };
    if let Some(forms) = schema.get("anyOf").and_then(Value::as_array) {
        if !forms.iter().any(|f| check_value(f, value, path).is_ok()) {
            let expected = forms
                .iter()
                .map(|f| {
                    f.get("type")
                        .map(describe_type)
                        .unwrap_or_else(|| "a value".into())
                })
                .collect::<Vec<_>>()
                .join(" or ");
            return Err(invalid(expected));
        }
    }
    if let Some(ty) = schema.get("type") {
        if !type_matches(ty, value) {
            return Err(invalid(describe_type(ty)));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            let options = allowed
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(invalid(format!("one of {options}")));
        }
    }
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
        if value.as_f64().is_some_and(|v| v < minimum) {
            return Err(invalid(format!("at least {minimum}")));
        }
    }
    match value {
        Value::Object(obj) => check_object(schema, obj, path),
        Value::Array(items) => match schema.get("items") {
            Some(item) => items
                .iter()
                .enumerate()
                .try_for_each(|(i, v)| check_value(item, v, &format!("{path}[{i}]"))),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Whether `value` has JSON Schema type `ty` (a name or a list of names).
/// An integer is what handlers read with `as_i64`/`as_u64`: `3`, not `3.0`.
fn type_matches(ty: &Value, value: &Value) -> bool {
    match ty {
        Value::String(name) => match name.as_str() {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            _ => false,
        },
        Value::Array(names) => names.iter().any(|n| type_matches(n, value)),
        _ => false,
    }
}

fn describe_type(ty: &Value) -> String {
    let one = |name: &str| match name {
        "integer" => "an integer".to_string(),
        "object" => "an object".to_string(),
        "array" => "an array".to_string(),
        "null" => "null".to_string(),
        other => format!("a {other}"),
    };
    match ty {
        Value::String(name) => one(name),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .map(one)
            .collect::<Vec<_>>()
            .join(" or "),
        _ => "a value".to_string(),
    }
}

#[cfg(test)]
#[path = "mcp_schema_tests.rs"]
mod tests;
