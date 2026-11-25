use super::*;

/// The served spec reports this build's version, whatever the file says.
#[test]
fn served_openapi_spec_reports_the_build_version() {
    let spec: serde_yaml::Value = serde_yaml::from_str(&OPENAPI_YAML_VERSIONED).unwrap();
    assert_eq!(
        spec["info"]["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "info.version must come from the crate version"
    );
    // Everything else is untouched: same title, same paths.
    let original: serde_yaml::Value = serde_yaml::from_str(OPENAPI_YAML).unwrap();
    assert_eq!(spec["info"]["title"], original["info"]["title"]);
    assert_eq!(spec["paths"], original["paths"]);
}

/// Only the `version` directly under `info:` changes; nested `version` keys
/// (a schema property, a description line) are left alone.
#[test]
fn with_info_version_replaces_only_the_info_version() {
    let spec = "openapi: 3.0.3\ninfo:\n  title: T\n  description: |\n    version: not this one\n  version: \"0.0.1\"\npaths: {}\ncomponents:\n  schemas:\n    S:\n      properties:\n        version: { type: string }\n";
    let out = with_info_version(spec, "9.9.9");
    assert!(out.contains("  version: \"9.9.9\"\n"), "{out}");
    assert!(out.contains("    version: not this one\n"), "{out}");
    assert!(out.contains("        version: { type: string }\n"), "{out}");
    assert!(!out.contains("0.0.1"), "{out}");
    assert_eq!(out.matches("9.9.9").count(), 1, "{out}");
}

/// A spec without an `info.version` is returned unchanged.
#[test]
fn with_info_version_leaves_a_spec_without_one_unchanged() {
    let spec = "openapi: 3.0.3\ninfo:\n  title: T\npaths: {}\n";
    assert_eq!(with_info_version(spec, "9.9.9"), spec);
}
