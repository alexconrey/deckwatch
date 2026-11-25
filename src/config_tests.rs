// Unit tests for src/config.rs

use super::*;
use clap::{CommandFactory as _, Parser};

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
fn cache_dir_takes_a_path_or_an_empty_value() {
    let cfg = Config::try_parse_from(["deckwatch", "--cache-dir", "/var/cache/deckwatch"]).unwrap();
    assert_eq!(
        cfg.cache_dir.as_deref(),
        Some(std::ffi::OsStr::new("/var/cache/deckwatch"))
    );
    // Empty turns the cache off (`plugin_cache`); it mustn't stop startup.
    let cfg = Config::try_parse_from(["deckwatch", "--cache-dir", ""]).unwrap();
    assert_eq!(cfg.cache_dir.as_deref(), Some(std::ffi::OsStr::new("")));
}

/// #301: plugin calls had no time limit unless one was configured, and
/// nothing configured one.
#[test]
fn plugin_calls_have_a_60_second_time_limit_by_default() {
    let cfg = Config::try_parse_from(["deckwatch"]).unwrap();
    assert_eq!(cfg.plugin_call_timeout, Duration::from_secs(60));
}

#[test]
fn the_plugin_call_time_limit_is_whole_seconds_from_1_to_3600() {
    for (value, secs) in [("1", 1), ("90", 90), (" 120 ", 120), ("3600", 3600)] {
        let cfg = Config::try_parse_from(["deckwatch", "--plugin-call-timeout", value]).unwrap();
        assert_eq!(
            cfg.plugin_call_timeout,
            Duration::from_secs(secs),
            "{value:?}"
        );
    }
}

#[test]
fn the_plugin_call_time_limit_is_read_from_deckwatch_plugin_call_timeout() {
    let command = Config::command();
    let arg = command
        .get_arguments()
        .find(|a| a.get_id() == "plugin_call_timeout")
        .expect("the --plugin-call-timeout argument");
    assert_eq!(
        arg.get_env(),
        Some(std::ffi::OsStr::new("DECKWATCH_PLUGIN_CALL_TIMEOUT"))
    );
}

/// Plugin calls always have a time limit: anything but 1 to 3600 seconds,
/// 0 and empty included, stops deckwatch at startup with an error that names
/// the variable (`Config::parse` prints it and exits non-zero).
#[test]
fn deckwatch_does_not_start_without_a_usable_plugin_call_time_limit() {
    let invalid = [
        "0",
        "",
        "  ",
        "3601",
        "18446744073709551616",
        "-1",
        "1.5",
        "60s",
        "1m",
        "none",
    ];
    for value in invalid {
        // `=` so that `-1` is read as the value, not as a flag.
        let flag = format!("--plugin-call-timeout={value}");
        let err = Config::try_parse_from(["deckwatch", flag.as_str()]).expect_err(value);
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "{err}");
        assert_ne!(err.exit_code(), 0);
        let message = err.to_string();
        assert!(
            message.contains("DECKWATCH_PLUGIN_CALL_TIMEOUT"),
            "{message}"
        );
        assert!(message.contains("from 1 to 3600"), "{message}");
    }

    // 0 isn't "no limit", as it is for the audit limits; the error says so.
    let err = Config::try_parse_from(["deckwatch", "--plugin-call-timeout=0"]).unwrap_err();
    assert!(
        err.to_string()
            .contains("can't be 0: plugin calls always have a time limit"),
        "{err}"
    );
}

#[test]
fn frontend_dir_override() {
    let cfg =
        Config::try_parse_from(["deckwatch", "--frontend-dir", "/opt/deckwatch/dist"]).unwrap();
    assert_eq!(cfg.frontend_dir, "/opt/deckwatch/dist");
}
