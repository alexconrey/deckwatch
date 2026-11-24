// Unit tests for src/metrics.rs

use super::*;

#[test]
fn label_set_caps_distinct_values() {
    let set = LabelSet::default();
    assert_eq!(set.bound("a".into(), 2), "a");
    assert_eq!(set.bound("b".into(), 2), "b");
    // Cap reached: new values collapse, known values keep reporting.
    assert_eq!(set.bound("c".into(), 2), OTHER_LABEL);
    assert_eq!(set.bound("a".into(), 2), "a");
    assert_eq!(set.bound("b".into(), 2), "b");
}

#[test]
fn label_set_rejects_malformed_values() {
    let set = LabelSet::default();
    for bad in [
        String::new(),
        "x".repeat(MAX_FRONTEND_LABEL_LEN + 1),
        "has space".to_string(),
        "new\nline".to_string(),
        "ünïcode".to_string(),
    ] {
        assert_eq!(set.bound(bad.clone(), 100), OTHER_LABEL, "{bad:?}");
    }
    assert_eq!(
        set.bound("/api/namespaces/:ns/deployments".into(), 100),
        "/api/namespaces/:ns/deployments"
    );
    // Malformed values don't consume capacity.
    assert_eq!(set.0.lock().unwrap().len(), 1);
}

#[test]
fn enumerable_labels_map_to_fixed_sets() {
    assert_eq!(method_label("get"), "GET");
    assert_eq!(method_label("DELETE"), "DELETE");
    assert_eq!(method_label("PROPFIND"), OTHER_LABEL);
    assert_eq!(status_class_label(204), "2xx");
    assert_eq!(status_class_label(503), "5xx");
    assert_eq!(status_class_label(0), OTHER_LABEL);
    assert_eq!(status_class_label(65535), OTHER_LABEL);
    assert_eq!(web_vital_label("LCP"), "LCP");
    assert_eq!(web_vital_label("totally-made-up"), OTHER_LABEL);
}
