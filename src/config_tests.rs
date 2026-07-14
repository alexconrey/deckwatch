// Unit tests for src/config.rs

use super::*;
use clap::Parser;

#[test]
fn defaults_are_reasonable() {
    // Parsing with no args should succeed and produce the documented defaults.
    let cfg = Config::try_parse_from(["deckwatch"]).unwrap();
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.frontend_dir, "frontend/dist");
    // Empty default expands to a single empty string entry that is filtered out.
    assert!(cfg.allowed_namespaces().is_empty());
}

#[test]
fn namespaces_parsed_as_comma_delimited() {
    let cfg =
        Config::try_parse_from(["deckwatch", "--namespaces", "team-a,team-b,team-c"]).unwrap();
    let allowed = cfg.allowed_namespaces();
    assert_eq!(allowed, vec!["team-a", "team-b", "team-c"]);
}

#[test]
fn allowed_namespaces_filters_empty_entries() {
    // Trailing/leading commas produce empty strings — they must be dropped so
    // `is_namespace_allowed("")` doesn't accidentally return true.
    let cfg = Config::try_parse_from(["deckwatch", "--namespaces", "team-a,,team-b,"]).unwrap();
    assert_eq!(cfg.allowed_namespaces(), vec!["team-a", "team-b"]);
}

#[test]
fn port_override() {
    let cfg = Config::try_parse_from(["deckwatch", "--port", "9090"]).unwrap();
    assert_eq!(cfg.port, 9090);
}

#[test]
fn invalid_port_rejected() {
    assert!(Config::try_parse_from(["deckwatch", "--port", "not-a-number"]).is_err());
}

#[test]
fn frontend_dir_override() {
    let cfg =
        Config::try_parse_from(["deckwatch", "--frontend-dir", "/opt/deckwatch/dist"]).unwrap();
    assert_eq!(cfg.frontend_dir, "/opt/deckwatch/dist");
}
