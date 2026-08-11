// Unit tests for types and helpers in src/handlers/templates.rs

use super::*;

// ---- DeploymentTemplate serde roundtrip ----

#[test]
fn deployment_template_roundtrip() {
    let template = DeploymentTemplate {
        id: "my-template".to_string(),
        name: "My Template".to_string(),
        description: "A custom template".to_string(),
        icon: "mdi-rocket".to_string(),
        category: TemplateCategory::Worker,
        payload: serde_json::json!({"image": "busybox:latest", "replicas": 2}),
        builtin: false,
    };
    let json = serde_json::to_string(&template).unwrap();
    let roundtripped: DeploymentTemplate = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtripped.id, "my-template");
    assert_eq!(roundtripped.name, "My Template");
    assert_eq!(roundtripped.description, "A custom template");
    assert_eq!(roundtripped.icon, "mdi-rocket");
    assert_eq!(roundtripped.payload["replicas"], 2);
}

#[test]
fn deployment_template_builtin_false_is_omitted() {
    let template = DeploymentTemplate {
        id: "t".to_string(),
        name: "T".to_string(),
        description: "d".to_string(),
        icon: "i".to_string(),
        category: TemplateCategory::WebApp,
        payload: serde_json::json!({}),
        builtin: false,
    };
    let json = serde_json::to_value(&template).unwrap();
    assert!(
        !json.as_object().unwrap().contains_key("builtin"),
        "builtin=false should be skipped via skip_serializing_if"
    );
}

#[test]
fn deployment_template_builtin_true_is_present() {
    let template = DeploymentTemplate {
        id: "t".to_string(),
        name: "T".to_string(),
        description: "d".to_string(),
        icon: "i".to_string(),
        category: TemplateCategory::WebApp,
        payload: serde_json::json!({}),
        builtin: true,
    };
    let json = serde_json::to_value(&template).unwrap();
    assert_eq!(json["builtin"], true);
}

#[test]
fn deployment_template_missing_builtin_defaults_to_false() {
    let json = r#"{
        "id": "x",
        "name": "X",
        "description": "d",
        "icon": "i",
        "category": "web_app",
        "payload": {}
    }"#;
    let t: DeploymentTemplate = serde_json::from_str(json).unwrap();
    assert!(!t.builtin);
}

// ---- TemplateCategory enum ----

#[test]
fn template_category_serializes_snake_case() {
    let val = serde_json::to_value(TemplateCategory::WebApp).unwrap();
    assert_eq!(val, "web_app");

    let val = serde_json::to_value(TemplateCategory::CronJob).unwrap();
    assert_eq!(val, "cron_job");

    let val = serde_json::to_value(TemplateCategory::Worker).unwrap();
    assert_eq!(val, "worker");

    let val = serde_json::to_value(TemplateCategory::StaticSite).unwrap();
    assert_eq!(val, "static_site");
}

#[test]
fn template_category_deserializes_snake_case() {
    let cat: TemplateCategory = serde_json::from_str(r#""web_app""#).unwrap();
    assert!(matches!(cat, TemplateCategory::WebApp));

    let cat: TemplateCategory = serde_json::from_str(r#""cron_job""#).unwrap();
    assert!(matches!(cat, TemplateCategory::CronJob));

    let cat: TemplateCategory = serde_json::from_str(r#""static_site""#).unwrap();
    assert!(matches!(cat, TemplateCategory::StaticSite));
}

// ---- TemplateListResponse serialization ----

#[test]
fn template_list_response_serializes_empty() {
    let resp = TemplateListResponse { templates: vec![] };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json["templates"].is_array());
    assert_eq!(json["templates"].as_array().unwrap().len(), 0);
}

#[test]
fn template_list_response_serializes_with_entries() {
    let resp = TemplateListResponse {
        templates: vec![DeploymentTemplate {
            id: "web-app".to_string(),
            name: "Web App".to_string(),
            description: "desc".to_string(),
            icon: "mdi-web".to_string(),
            category: TemplateCategory::WebApp,
            payload: serde_json::json!({"name": ""}),
            builtin: true,
        }],
    };
    let json = serde_json::to_value(&resp).unwrap();
    let arr = json["templates"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "web-app");
    assert_eq!(arr[0]["builtin"], true);
}

// ---- templates_equal ----

#[test]
fn templates_equal_detects_identical() {
    let a = DeploymentTemplate {
        id: "t1".to_string(),
        name: "T1".to_string(),
        description: "d".to_string(),
        icon: "i".to_string(),
        category: TemplateCategory::Worker,
        payload: serde_json::json!({"k": "v"}),
        builtin: false,
    };
    let b = a.clone();
    assert!(templates_equal(&a, &b));
}

#[test]
fn templates_equal_ignores_builtin_flag() {
    let a = DeploymentTemplate {
        id: "t1".to_string(),
        name: "T1".to_string(),
        description: "d".to_string(),
        icon: "i".to_string(),
        category: TemplateCategory::Worker,
        payload: serde_json::json!({}),
        builtin: true,
    };
    let mut b = a.clone();
    b.builtin = false;
    assert!(templates_equal(&a, &b));
}

#[test]
fn templates_equal_detects_name_diff() {
    let a = DeploymentTemplate {
        id: "t1".to_string(),
        name: "T1".to_string(),
        description: "d".to_string(),
        icon: "i".to_string(),
        category: TemplateCategory::Worker,
        payload: serde_json::json!({}),
        builtin: false,
    };
    let mut b = a.clone();
    b.name = "Different".to_string();
    assert!(!templates_equal(&a, &b));
}

#[test]
fn templates_equal_detects_payload_diff() {
    let a = DeploymentTemplate {
        id: "t1".to_string(),
        name: "T1".to_string(),
        description: "d".to_string(),
        icon: "i".to_string(),
        category: TemplateCategory::Worker,
        payload: serde_json::json!({"replicas": 1}),
        builtin: false,
    };
    let mut b = a.clone();
    b.payload = serde_json::json!({"replicas": 3});
    assert!(!templates_equal(&a, &b));
}

// ---- merge_catalog (existing tests migrated + new) ----

#[test]
fn merge_appends_custom_entries() {
    let defaults = default_catalog();
    let defaults_len = defaults.len();
    let custom = vec![DeploymentTemplate {
        id: "custom-thing".to_string(),
        name: "Custom".to_string(),
        description: "d".to_string(),
        icon: "mdi-star".to_string(),
        category: TemplateCategory::WebApp,
        payload: serde_json::json!({"name": ""}),
        builtin: false,
    }];
    let merged = merge_catalog(defaults, custom);
    assert_eq!(merged.len(), defaults_len + 1);
    assert_eq!(merged.last().unwrap().id, "custom-thing");
    assert!(!merged.last().unwrap().builtin);
}

#[test]
fn merge_overrides_builtin_in_place() {
    let defaults = default_catalog();
    let overrides = vec![DeploymentTemplate {
        id: "web-app".to_string(),
        name: "Custom Web App".to_string(),
        description: "d".to_string(),
        icon: "mdi-web".to_string(),
        category: TemplateCategory::WebApp,
        payload: serde_json::json!({"image": "custom:latest"}),
        builtin: false,
    }];
    let merged = merge_catalog(defaults, overrides);
    let web = merged.iter().find(|t| t.id == "web-app").unwrap();
    assert_eq!(web.name, "Custom Web App");
    assert!(web.builtin, "overridden default should retain builtin=true");
}

#[test]
fn merge_with_no_overrides_returns_defaults() {
    let defaults = default_catalog();
    let expected_len = defaults.len();
    let merged = merge_catalog(defaults, vec![]);
    assert_eq!(merged.len(), expected_len);
    assert!(merged.iter().all(|t| t.builtin));
}

#[test]
fn merge_preserves_default_order() {
    let defaults = default_catalog();
    let default_ids: Vec<String> = defaults.iter().map(|t| t.id.clone()).collect();
    let merged = merge_catalog(defaults, vec![]);
    let merged_ids: Vec<String> = merged.iter().map(|t| t.id.clone()).collect();
    assert_eq!(merged_ids, default_ids);
}

// ---- default_catalog ----

#[test]
fn default_catalog_has_expected_ids() {
    let catalog = default_catalog();
    let ids: Vec<&str> = catalog.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"web-app"));
    assert!(ids.contains(&"worker"));
    assert!(ids.contains(&"cron-job"));
    assert!(ids.contains(&"static-site"));
}

#[test]
fn default_catalog_entries_are_builtin() {
    let catalog = default_catalog();
    assert!(
        catalog.iter().all(|t| t.builtin),
        "all default catalog entries must be builtin=true"
    );
}
