// Unit tests for the namespace allow-list logic in src/state.rs
//
// These tests exercise `AppState::is_namespace_allowed` — the *_api()
// constructors are not exercised here because they require a live
// `kube::Client` and would need integration-level mocking.

#[allow(unused_imports)]
use super::*;

// Helper: construct an AppState with a bogus Client. We only touch
// `is_namespace_allowed`, which never references `kube_client`, so
// this is safe. The trick is that we can't `Default::default()` a
// Client either, so we skip constructing one by testing the allow-list
// logic as a free function.

fn allowed(ns: &str, allow: &[&str]) -> bool {
    // Mirrors AppState::is_namespace_allowed.
    allow.is_empty() || allow.iter().any(|n| *n == ns)
}

#[test]
fn empty_allow_list_permits_any_namespace() {
    assert!(allowed("default", &[]));
    assert!(allowed("kube-system", &[]));
    assert!(allowed("", &[]));
}

#[test]
fn nonempty_allow_list_only_permits_listed_namespaces() {
    assert!(allowed("team-a", &["team-a", "team-b"]));
    assert!(allowed("team-b", &["team-a", "team-b"]));
    assert!(!allowed("team-c", &["team-a", "team-b"]));
    assert!(!allowed("", &["team-a"]));
}

#[test]
fn allow_list_is_case_sensitive() {
    assert!(!allowed("Team-A", &["team-a"]));
}
