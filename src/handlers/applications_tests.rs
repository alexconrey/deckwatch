use super::*;

// ---- ApplicationRequest deserialization ----

#[test]
fn application_request_deserializes_minimal() {
    let json = r#"{"name": "my-app"}"#;
    let req: ApplicationRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.name, "my-app");
    assert!(req.description.is_none());
    assert!(req.git.is_none());
    assert!(req.create_deployment.is_none());
    assert!(req.template_id.is_none());
}

#[test]
fn application_request_deserializes_full() {
    let json = r#"{
        "name": "web-frontend",
        "description": "The main web frontend",
        "git": {
            "repo_url": "https://github.com/org/repo",
            "branch": "develop",
            "token_secret": "git-creds"
        },
        "create_deployment": true,
        "template_id": "static-site"
    }"#;
    let req: ApplicationRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.name, "web-frontend");
    assert_eq!(req.description.as_deref(), Some("The main web frontend"));
    assert!(req.create_deployment.unwrap());
    assert_eq!(req.template_id.as_deref(), Some("static-site"));

    let git = req.git.unwrap();
    assert_eq!(git.repo_url, "https://github.com/org/repo");
    assert_eq!(git.branch.as_deref(), Some("develop"));
    assert_eq!(git.token_secret.as_deref(), Some("git-creds"));
}

// ---- ApplicationUpdateRequest deserialization ----

#[test]
fn application_update_request_empty_body() {
    let json = r#"{}"#;
    let req: ApplicationUpdateRequest = serde_json::from_str(json).unwrap();
    assert!(req.description.is_none());
    assert!(req.git.is_none());
}

#[test]
fn application_update_request_with_description_only() {
    let json = r#"{"description": "updated desc"}"#;
    let req: ApplicationUpdateRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.description.as_deref(), Some("updated desc"));
    assert!(req.git.is_none());
}

#[test]
fn application_update_request_with_git_null_clears_git() {
    let json = r#"{"git": null}"#;
    let req: ApplicationUpdateRequest = serde_json::from_str(json).unwrap();
    let outer = req.git.expect("outer Option should be Some");
    assert!(outer.is_none(), "inner Option should be None (clear)");
}

#[test]
fn application_update_request_with_git_value() {
    let json = r#"{
        "git": {
            "repo_url": "https://example.com/repo.git"
        }
    }"#;
    let req: ApplicationUpdateRequest = serde_json::from_str(json).unwrap();
    let outer = req.git.expect("outer Option should be Some");
    let git = outer.expect("inner Option should be Some");
    assert_eq!(git.repo_url, "https://example.com/repo.git");
}

// ---- AddMemberRequest deserialization ----

#[test]
fn add_member_request_deployment() {
    let json = r#"{"kind": "Deployment", "resource_name": "api-server"}"#;
    let req: AddMemberRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.kind, "Deployment");
    assert_eq!(req.resource_name, "api-server");
}

#[test]
fn add_member_request_cronjob() {
    let json = r#"{"kind": "CronJob", "resource_name": "nightly-report"}"#;
    let req: AddMemberRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.kind, "CronJob");
    assert_eq!(req.resource_name, "nightly-report");
}

// ---- DeleteQuery deserialization ----

#[test]
fn delete_query_defaults_cascade_to_none() {
    let json = r#"{}"#;
    let q: DeleteQuery = serde_json::from_str(json).unwrap();
    assert!(q.cascade.is_none());
}

#[test]
fn delete_query_cascade_true() {
    let json = r#"{"cascade": true}"#;
    let q: DeleteQuery = serde_json::from_str(json).unwrap();
    assert_eq!(q.cascade, Some(true));
}

// ---- ApplicationListResponse serialization ----

#[test]
fn application_list_response_serializes() {
    let resp = ApplicationListResponse {
        applications: vec![],
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json["applications"].is_array());
    assert_eq!(json["applications"].as_array().unwrap().len(), 0);
}

// ---- Helper functions ----

#[test]
fn member_selector_formats_correctly() {
    assert_eq!(member_selector("my-app"), "deckwatch.io/application=my-app");
}

// ---- seed_template ----

#[test]
fn seed_template_web_app_default() {
    let tmpl = seed_template("web-app");
    assert_eq!(tmpl.image, "nginx:1.27-alpine");
    assert_eq!(tmpl.replicas, 1);
    assert_eq!(tmpl.port, Some(80));
    assert!(tmpl.readiness_path.is_some());
    assert!(tmpl.command.is_none());
}

#[test]
fn seed_template_worker() {
    let tmpl = seed_template("worker");
    assert!(tmpl.image.is_empty());
    assert_eq!(tmpl.replicas, 1);
    assert!(tmpl.port.is_none());
    assert!(tmpl.readiness_path.is_none());
}

#[test]
fn seed_template_cron_job() {
    let tmpl = seed_template("cron-job");
    assert_eq!(tmpl.replicas, 0);
    assert!(tmpl.command.is_some());
    assert!(tmpl.args.is_some());
    assert!(tmpl.port.is_none());
}

#[test]
fn seed_template_static_site() {
    let tmpl = seed_template("static-site");
    assert_eq!(tmpl.image, "nginx:1.27-alpine");
    assert_eq!(tmpl.port, Some(80));
    assert!(tmpl.readiness_path.is_some());
}

#[test]
fn seed_template_unknown_falls_back_to_web_app() {
    let tmpl = seed_template("nonexistent-template");
    assert_eq!(tmpl.image, "nginx:1.27-alpine");
    assert_eq!(tmpl.replicas, 1);
    assert_eq!(tmpl.port, Some(80));
}

// ---- build_resources ----

#[test]
fn build_resources_all_none_returns_none() {
    assert!(build_resources(None, None, None, None).is_none());
}

#[test]
fn build_resources_with_requests_only() {
    let res = build_resources(
        Some("100m".to_string()),
        Some("128Mi".to_string()),
        None,
        None,
    )
    .expect("should produce ResourceRequirements");
    let reqs = res.requests.unwrap();
    assert_eq!(reqs.get("cpu").unwrap().0, "100m");
    assert_eq!(reqs.get("memory").unwrap().0, "128Mi");
    assert!(res.limits.is_none());
}

#[test]
fn build_resources_with_limits_only() {
    let res = build_resources(None, None, Some("1".to_string()), Some("512Mi".to_string()))
        .expect("should produce ResourceRequirements");
    assert!(res.requests.is_none());
    let lims = res.limits.unwrap();
    assert_eq!(lims.get("cpu").unwrap().0, "1");
    assert_eq!(lims.get("memory").unwrap().0, "512Mi");
}

#[test]
fn build_resources_with_both() {
    let res = build_resources(
        Some("100m".to_string()),
        Some("128Mi".to_string()),
        Some("500m".to_string()),
        Some("256Mi".to_string()),
    )
    .expect("should produce ResourceRequirements");
    assert!(res.requests.is_some());
    assert!(res.limits.is_some());
}
