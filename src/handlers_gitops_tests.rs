use super::*;
use serde_json;

// ---- GitOpsConfigRequest deserialization ----

#[test]
fn config_request_deserializes_all_fields() {
    let json = serde_json::json!({
        "repo_url": "https://github.com/org/repo.git",
        "branch": "develop",
        "token_secret": "my-token-secret",
        "dockerfile_path": "docker/Dockerfile.prod",
        "docker_context": "./app",
        "oci_repository": "591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/my-app",
        "ecr_repository": "591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/legacy",
        "include_paths": ["src/", "Cargo.toml"],
        "exclude_paths": ["tests/", "docs/"],
        "poll_interval_seconds": 120,
        "webhook_enabled": true,
        "webhook_secret": "supersecret123"
    });

    let req: GitOpsConfigRequest = serde_json::from_value(json).expect("deserialize");
    assert_eq!(req.repo_url, "https://github.com/org/repo.git");
    assert_eq!(req.branch.as_deref(), Some("develop"));
    assert_eq!(req.token_secret.as_deref(), Some("my-token-secret"));
    assert_eq!(
        req.dockerfile_path.as_deref(),
        Some("docker/Dockerfile.prod")
    );
    assert_eq!(req.docker_context.as_deref(), Some("./app"));
    assert_eq!(
        req.oci_repository.as_deref(),
        Some("591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/my-app")
    );
    assert_eq!(
        req.ecr_repository.as_deref(),
        Some("591839118651.dkr.ecr.us-gov-west-1.amazonaws.com/apps/legacy")
    );
    assert_eq!(
        req.include_paths.as_deref(),
        Some(&["src/".to_string(), "Cargo.toml".to_string()][..])
    );
    assert_eq!(
        req.exclude_paths.as_deref(),
        Some(&["tests/".to_string(), "docs/".to_string()][..])
    );
    assert_eq!(req.poll_interval_seconds, Some(120));
    assert_eq!(req.webhook_enabled, Some(true));
    assert_eq!(req.webhook_secret.as_deref(), Some("supersecret123"));
}

#[test]
fn config_request_deserializes_minimal_fields() {
    let json = serde_json::json!({
        "repo_url": "https://github.com/org/repo.git"
    });

    let req: GitOpsConfigRequest = serde_json::from_value(json).expect("deserialize");
    assert_eq!(req.repo_url, "https://github.com/org/repo.git");
    assert!(req.branch.is_none());
    assert!(req.token_secret.is_none());
    assert!(req.dockerfile_path.is_none());
    assert!(req.docker_context.is_none());
    assert!(req.oci_repository.is_none());
    assert!(req.ecr_repository.is_none());
    assert!(req.include_paths.is_none());
    assert!(req.exclude_paths.is_none());
    assert!(req.poll_interval_seconds.is_none());
    assert!(req.webhook_enabled.is_none());
    assert!(req.webhook_secret.is_none());
}

#[test]
fn config_request_rejects_missing_repo_url() {
    let json = serde_json::json!({
        "branch": "main"
    });
    let result = serde_json::from_value::<GitOpsConfigRequest>(json);
    assert!(result.is_err(), "repo_url is required by the struct");
}

// ---- GitOpsStatusResponse serialization ----

#[test]
fn status_response_serializes_enabled_with_config() {
    let config = GitOpsConfig {
        repo_url: "https://github.com/org/repo.git".to_string(),
        branch: "main".to_string(),
        token_secret: Some("my-secret".to_string()),
        dockerfile_path: "Dockerfile".to_string(),
        docker_context: ".".to_string(),
        oci_repository: "registry.example.com/app".to_string(),
        ecr_repository: "registry.example.com/app".to_string(),
        include_paths: vec!["src/".to_string()],
        exclude_paths: vec![],
        poll_interval_seconds: 60,
        webhook_enabled: true,
        webhook_secret_configured: true,
    };

    let resp = GitOpsStatusResponse {
        enabled: true,
        config: Some(config),
        last_commit_sha: Some("abc1234".to_string()),
        last_build_status: Some("success".to_string()),
        last_build_job: Some("build-job-1".to_string()),
        last_build_time: Some("2025-01-01T00:00:00Z".to_string()),
        last_build_error: None,
    };

    let value = serde_json::to_value(&resp).expect("serialize");
    assert_eq!(value["enabled"], true);
    assert_eq!(
        value["config"]["repo_url"],
        "https://github.com/org/repo.git"
    );
    assert_eq!(value["config"]["branch"], "main");
    assert_eq!(value["config"]["token_secret"], "my-secret");
    assert_eq!(
        value["config"]["oci_repository"],
        "registry.example.com/app"
    );
    assert_eq!(
        value["config"]["ecr_repository"],
        "registry.example.com/app"
    );
    assert_eq!(value["config"]["poll_interval_seconds"], 60);
    assert_eq!(value["config"]["webhook_enabled"], true);
    assert_eq!(value["config"]["webhook_secret_configured"], true);
    assert_eq!(
        value["config"]["include_paths"],
        serde_json::json!(["src/"])
    );
    assert_eq!(value["config"]["exclude_paths"], serde_json::json!([]));
    assert_eq!(value["last_commit_sha"], "abc1234");
    assert_eq!(value["last_build_status"], "success");
    assert_eq!(value["last_build_job"], "build-job-1");
    assert_eq!(value["last_build_time"], "2025-01-01T00:00:00Z");
    assert!(value["last_build_error"].is_null());
}

#[test]
fn status_response_serializes_disabled_without_config() {
    let resp = GitOpsStatusResponse {
        enabled: false,
        config: None,
        last_commit_sha: None,
        last_build_status: None,
        last_build_job: None,
        last_build_time: None,
        last_build_error: None,
    };

    let value = serde_json::to_value(&resp).expect("serialize");
    assert_eq!(value["enabled"], false);
    assert!(value["config"].is_null());
    assert!(value["last_commit_sha"].is_null());
    assert!(value["last_build_status"].is_null());
    assert!(value["last_build_job"].is_null());
    assert!(value["last_build_time"].is_null());
    assert!(value["last_build_error"].is_null());
}

// ---- GitOpsConfig serialization ----

#[test]
fn gitops_config_token_secret_none_serializes_as_null() {
    let config = GitOpsConfig {
        repo_url: "https://github.com/org/repo.git".to_string(),
        branch: "main".to_string(),
        token_secret: None,
        dockerfile_path: "Dockerfile".to_string(),
        docker_context: ".".to_string(),
        oci_repository: "reg/app".to_string(),
        ecr_repository: "reg/app".to_string(),
        include_paths: vec![],
        exclude_paths: vec![],
        poll_interval_seconds: 30,
        webhook_enabled: false,
        webhook_secret_configured: false,
    };

    let value = serde_json::to_value(&config).expect("serialize");
    assert!(value["token_secret"].is_null());
    assert_eq!(value["webhook_enabled"], false);
    assert_eq!(value["webhook_secret_configured"], false);
}

// ---- BuildSummary / BuildListResponse serialization ----

#[test]
fn build_summary_serializes_all_fields() {
    let summary = BuildSummary {
        job_name: "build-abc1234".to_string(),
        commit_sha: "abc1234def5678".to_string(),
        status: "success".to_string(),
        started_at: Some("2025-06-01T12:00:00Z".to_string()),
        completed_at: Some("2025-06-01T12:05:00Z".to_string()),
        image_tag: "abc1234".to_string(),
    };

    let value = serde_json::to_value(&summary).expect("serialize");
    assert_eq!(value["job_name"], "build-abc1234");
    assert_eq!(value["commit_sha"], "abc1234def5678");
    assert_eq!(value["status"], "success");
    assert_eq!(value["started_at"], "2025-06-01T12:00:00Z");
    assert_eq!(value["completed_at"], "2025-06-01T12:05:00Z");
    assert_eq!(value["image_tag"], "abc1234");
}

#[test]
fn build_summary_serializes_optional_timestamps_as_null() {
    let summary = BuildSummary {
        job_name: "build-xyz".to_string(),
        commit_sha: "deadbeef".to_string(),
        status: "building".to_string(),
        started_at: None,
        completed_at: None,
        image_tag: "deadbee".to_string(),
    };

    let value = serde_json::to_value(&summary).expect("serialize");
    assert!(value["started_at"].is_null());
    assert!(value["completed_at"].is_null());
}

#[test]
fn build_list_response_serializes_empty_list() {
    let resp = BuildListResponse { builds: vec![] };
    let value = serde_json::to_value(&resp).expect("serialize");
    assert_eq!(value["builds"], serde_json::json!([]));
}

#[test]
fn build_list_response_serializes_multiple_builds() {
    let resp = BuildListResponse {
        builds: vec![
            BuildSummary {
                job_name: "build-1".to_string(),
                commit_sha: "aaa".to_string(),
                status: "success".to_string(),
                started_at: Some("2025-01-01T00:00:00Z".to_string()),
                completed_at: Some("2025-01-01T00:05:00Z".to_string()),
                image_tag: "aaa".to_string(),
            },
            BuildSummary {
                job_name: "build-2".to_string(),
                commit_sha: "bbb".to_string(),
                status: "failed".to_string(),
                started_at: Some("2025-01-02T00:00:00Z".to_string()),
                completed_at: None,
                image_tag: "bbb".to_string(),
            },
        ],
    };

    let value = serde_json::to_value(&resp).expect("serialize");
    let builds = value["builds"].as_array().expect("builds array");
    assert_eq!(builds.len(), 2);
    assert_eq!(builds[0]["job_name"], "build-1");
    assert_eq!(builds[0]["status"], "success");
    assert_eq!(builds[1]["job_name"], "build-2");
    assert_eq!(builds[1]["status"], "failed");
    assert!(builds[1]["completed_at"].is_null());
}

// ---- webhook_secret_name ----

#[test]
fn webhook_secret_name_formats_correctly() {
    assert_eq!(webhook_secret_name("my-app"), "my-app-gitops-webhook");
    assert_eq!(webhook_secret_name("frontend"), "frontend-gitops-webhook");
}

#[test]
fn webhook_secret_name_handles_empty_deployment() {
    assert_eq!(webhook_secret_name(""), "-gitops-webhook");
}

// ---- now_utc ----

#[test]
fn now_utc_returns_reasonable_timestamp() {
    let ts = now_utc();
    // The timestamp should be after 2024-01-01 and before 2100-01-01.
    let year = ts.format("%Y").to_string();
    let y: i32 = year.parse().expect("valid year");
    assert!(y >= 2024, "timestamp year {y} should be >= 2024");
    assert!(y <= 2100, "timestamp year {y} should be <= 2100");
}

// ---- JobPodSummary / JobPodListResponse serialization ----

#[test]
fn job_pod_summary_serializes() {
    let summary = JobPodSummary {
        name: "build-abc-xyz".to_string(),
        phase: "Running".to_string(),
    };
    let value = serde_json::to_value(&summary).expect("serialize");
    assert_eq!(value["name"], "build-abc-xyz");
    assert_eq!(value["phase"], "Running");
}

#[test]
fn job_pod_list_response_serializes_empty() {
    let resp = JobPodListResponse { pods: vec![] };
    let value = serde_json::to_value(&resp).expect("serialize");
    assert_eq!(value["pods"], serde_json::json!([]));
}
