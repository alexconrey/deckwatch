//! `DECKWATCH_CACHE_DIR` resolution against real directories, with the
//! config loaded through extism the way plugin calls load it. Calls through
//! the plugin funnel are in `plugins_funnel_tests.rs`.

use super::*;
use crate::plugin_fixture::capture::{capture, CapturedEvent};

/// The lines this module logged.
fn ours(events: &[CapturedEvent]) -> Vec<&CapturedEvent> {
    events
        .iter()
        .filter(|e| e.target == "deckwatch::plugin_cache")
        .collect()
}

/// The one line this module logged, which must be at `level`.
fn one_line(events: &[CapturedEvent], level: tracing::Level) -> &CapturedEvent {
    let lines = ours(events);
    assert_eq!(lines.len(), 1, "{events:?}");
    assert_eq!(lines[0].level, level, "{:?}", lines[0]);
    lines[0]
}

#[test]
fn unset_or_empty_turns_the_cache_off_and_says_how_to_turn_it_on() {
    for dir in [None, Some(Path::new(""))] {
        let (cache, events) = capture(|| CompileCache::resolve(dir));
        assert_eq!(cache.config_file(), None);
        let line = one_line(&events, tracing::Level::INFO);
        assert!(line.message.contains("set DECKWATCH_CACHE_DIR"), "{line:?}");
    }
}

#[test]
fn a_writable_directory_turns_the_cache_on_with_a_config_wasmtime_loads() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, events) = capture(|| CompileCache::resolve(Some(dir.path())));
    one_line(&events, tracing::Level::INFO);

    let config = cache.config_file().expect("the cache is on");
    assert_eq!(config, dir.path().join("wasmtime-cache.toml"));
    let cache_dir = dir.path().join("wasmtime");
    let text = fs::read_to_string(config).unwrap();
    assert!(text.contains("\n[cache]\n"), "{text}");
    assert!(
        text.contains(&format!("\ndirectory = \"{}\"\n", cache_dir.display())),
        "{text}"
    );
    // Loading the config at startup compiled a module into the cache.
    assert!(fs::read_dir(&cache_dir).unwrap().next().is_some());
    // And the write check cleaned up after itself.
    let mut names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["wasmtime", "wasmtime-cache.toml"]);
}

#[test]
fn a_directory_deckwatch_cannot_create_turns_the_cache_off_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the directory should be: nobody can create a directory
    // under it, root included.
    let file = dir.path().join("not-a-directory");
    fs::write(&file, b"").unwrap();

    let (cache, events) = capture(|| CompileCache::resolve(Some(&file)));
    assert_eq!(cache.config_file(), None);
    let line = one_line(&events, tracing::Level::WARN);
    assert!(line.message.contains("DECKWATCH_CACHE_DIR is not usable"));
    assert!(line.fields["error"].contains("can't create"), "{line:?}");
}

#[cfg(unix)]
#[test]
fn a_directory_deckwatch_cannot_write_turns_the_cache_off_with_a_warning() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("wasmtime");
    fs::create_dir(&cache_dir).unwrap();
    fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o555)).unwrap();
    // Root writes regardless of the mode; there's nothing to check then.
    let privileged = fs::write(cache_dir.join("probe"), b"").is_ok();
    let (cache, events) = capture(|| CompileCache::resolve(Some(dir.path())));
    fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o755)).unwrap();
    if privileged {
        return;
    }

    assert_eq!(cache.config_file(), None);
    let line = one_line(&events, tracing::Level::WARN);
    assert!(line.fields["error"].contains("can't write to"), "{line:?}");
}

/// wasmtime parses the path back out of the config file and writes there.
#[cfg(unix)]
#[test]
fn quotes_and_backslashes_in_the_path_survive_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let odd = dir.path().join(r#"a "cache" \ dir"#);
    let cache = CompileCache::resolve(Some(&odd));
    assert!(cache.config_file().is_some());
    assert!(fs::read_dir(odd.join("wasmtime")).unwrap().next().is_some());
}

#[test]
fn the_startup_check_refuses_a_config_wasmtime_does_not_accept() {
    // Older wasmtime cache configs said `enabled = true`; wasmtime 49 refuses
    // the whole file over it.
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("old.toml");
    let directory = toml_string(dir.path().join("wasmtime").to_str().unwrap());
    fs::write(
        &config,
        format!("[cache]\nenabled = true\ndirectory = {directory}\n"),
    )
    .unwrap();
    let err = format!("{:#}", load(&config).unwrap_err());
    assert!(err.starts_with("wasmtime can't use"), "{err}");
    assert!(err.contains("enabled"), "{err}");
}

/// wasmtime reads the config file on every compile, so the cache can break
/// after startup. Compiles still work, and deckwatch warns once.
#[test]
fn a_cache_that_breaks_after_startup_turns_itself_off_with_one_warning() {
    let dir = tempfile::tempdir().unwrap();
    let cache = CompileCache::resolve(Some(dir.path()));
    fs::remove_file(cache.config_file().unwrap()).unwrap();

    let compile = || cache.compile(PluginBuilder::new(EMPTY_MODULE)).is_ok();
    let (compiled, events) = capture(|| [compile(), compile()]);
    assert_eq!(compiled, [true, true]);
    assert_eq!(cache.config_file(), None);
    let line = one_line(&events, tracing::Level::WARN);
    assert!(line.message.contains("stopped working"), "{line:?}");
}

/// A broken plugin fails with and without the cache. Its own error is
/// reported, and the cache stays on.
#[test]
fn a_module_that_does_not_compile_keeps_its_error_and_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = CompileCache::resolve(Some(dir.path()));
    let truncated: &[u8] = b"\0asm\x01\0\0\0\x01";

    let (result, events) = capture(|| cache.compile(PluginBuilder::new(truncated)));
    assert!(result.is_err());
    assert!(cache.config_file().is_some());
    assert!(ours(&events).is_empty(), "{events:?}");
}

#[test]
fn toml_strings_escape_quotes_backslashes_and_control_characters() {
    assert_eq!(
        toml_string("/var/cache/deckwatch"),
        r#""/var/cache/deckwatch""#
    );
    assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    assert_eq!(toml_string("a\tb\nc\u{7f}"), r#""a\u0009b\u000Ac\u007F""#);
}
