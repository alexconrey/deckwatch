// Tests for deleting the Kubernetes objects a plugin's deprovision() asks for.

use super::*;
use crate::entities::audit_log;
use crate::plugins::ResourceDeprovisionResult;
use std::collections::BTreeSet;

fn obj(api_version: &str, kind: &str, name: &str) -> KubernetesObjectRef {
    KubernetesObjectRef {
        api_version: api_version.to_string(),
        kind: kind.to_string(),
        name: name.to_string(),
    }
}

const TARGET: DeprovisionTarget<'static> = DeprovisionTarget {
    namespace: "team",
    application: "shop",
    plugin: "AWS",
    resource_id: "rds",
};

// ── Wire format (deckwatch-plugin-sdk v0.10.0) ──────────────────────────────

#[test]
fn deprovision_result_from_older_plugin_has_no_deletions() {
    let r: ResourceDeprovisionResult =
        serde_json::from_str(r#"{"errors":[],"message":"RDS kept"}"#).unwrap();
    assert!(r.delete_kubernetes_resources.is_empty());
    assert_eq!(r.message, "RDS kept");
}

#[test]
fn deprovision_result_reads_what_the_sdk_writes() {
    let json = r#"{
        "errors": [],
        "message": "Deleted IAM role",
        "delete_kubernetes_resources": [
            { "api_version": "batch/v1", "kind": "Job", "name": "shop-db-bootstrap-1a2b3c4d" },
            { "api_version": "v1", "kind": "ServiceAccount", "name": "shop-aws-sa" }
        ]
    }"#;
    let r: ResourceDeprovisionResult = serde_json::from_str(json).unwrap();
    assert_eq!(
        r.delete_kubernetes_resources,
        vec![
            obj("batch/v1", "Job", "shop-db-bootstrap-1a2b3c4d"),
            obj("v1", "ServiceAccount", "shop-aws-sa"),
        ]
    );
}

#[test]
fn object_ref_has_no_namespace_and_tolerates_missing_fields() {
    let json = r#"{"delete_kubernetes_resources": [
        { "api_version": "v1", "kind": "ServiceAccount", "name": "x", "namespace": "kube-system" },
        { "kind": "Job" }
    ]}"#;
    let r: ResourceDeprovisionResult = serde_json::from_str(json).unwrap();
    assert_eq!(
        r.delete_kubernetes_resources,
        vec![obj("v1", "ServiceAccount", "x"), obj("", "Job", "")]
    );
}

// ── Pure helpers ─────────────────────────────────────────────────────────────

#[test]
fn only_service_accounts_and_jobs_are_deletable() {
    assert!(deletable_kind("v1", "ServiceAccount").is_some());
    assert!(deletable_kind("batch/v1", "Job").is_some());
    for (api_version, kind) in [
        ("v1", "Secret"),
        ("v1", "ConfigMap"),
        ("apps/v1", "Deployment"),
        ("batch/v1", "CronJob"),
        ("batch/v1beta1", "Job"),
        ("v1", "serviceaccount"),
        ("v1", "Job"),
        ("", "ServiceAccount"),
        ("v1", ""),
    ] {
        assert!(
            deletable_kind(api_version, kind).is_none(),
            "{api_version}/{kind}"
        );
    }
    // Jobs are deleted before ServiceAccounts.
    assert!(deletable_kind("batch/v1", "Job") < deletable_kind("v1", "ServiceAccount"));
}

#[test]
fn dns1123_names() {
    for ok in ["shop-aws-sa", "a", "shop.db-bootstrap-1a2b", "0x"] {
        assert!(is_dns1123_subdomain(ok), "{ok}");
    }
    for bad in [
        "",
        "Shop",
        "shop_sa",
        "-shop",
        "shop-",
        "a..b",
        ".a",
        "x/y",
        "../kube-system/serviceaccounts/x",
        "shop sa",
        "shop%2Fx",
        &"a".repeat(254),
    ] {
        assert!(!is_dns1123_subdomain(bad), "{bad:?}");
    }
    assert!(is_dns1123_label("team"));
    assert!(!is_dns1123_label("team.prod"));
    assert!(!is_dns1123_label("team/../x"));
    assert!(!is_dns1123_label(&"a".repeat(64)));
}

fn deployment(name: &str, service_account: Option<&str>) -> Deployment {
    serde_json::from_value(serde_json::json!({
        "metadata": { "name": name, "namespace": "team" },
        "spec": {
            "selector": { "matchLabels": { "app": name } },
            "template": { "spec": {
                "serviceAccountName": service_account,
                "containers": [{ "name": "app", "image": "app:1" }],
            } },
        },
    }))
    .unwrap()
}

fn cronjob(name: &str, service_account: &str) -> CronJob {
    serde_json::from_value(serde_json::json!({
        "metadata": { "name": name, "namespace": "team" },
        "spec": {
            "schedule": "0 3 * * *",
            "jobTemplate": { "spec": { "template": { "spec": {
                "serviceAccount": service_account,
                "restartPolicy": "Never",
                "containers": [{ "name": "job", "image": "job:1" }],
            } } } },
        },
    }))
    .unwrap()
}

fn job(name: &str, service_account: &str) -> Job {
    serde_json::from_value(job_json(name, service_account)).unwrap()
}

fn job_json(name: &str, service_account: &str) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": { "name": name, "namespace": "team" },
        "spec": { "template": { "spec": {
            "serviceAccountName": service_account,
            "restartPolicy": "Never",
            "containers": [{ "name": "job", "image": "job:1" }],
        } } },
    })
}

#[test]
fn service_account_users_finds_pod_templates_that_use_it() {
    let mut leaving = deployment("old-web", Some("shop-aws-sa"));
    leaving.metadata.deletion_timestamp =
        Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
            k8s_openapi::jiff::Timestamp::UNIX_EPOCH,
        ));
    let deployments = vec![
        deployment("shop-web", Some("shop-aws-sa")),
        deployment("blog-web", Some("blog-sa")),
        deployment("plain", None),
        leaving,
    ];
    // CronJob uses the deprecated `serviceAccount` field.
    let cronjobs = vec![cronjob("shop-nightly", "shop-aws-sa")];
    let jobs = vec![job("shop-db-bootstrap-1a2b", "shop-db-bootstrap")];

    assert_eq!(
        service_account_users("shop-aws-sa", &deployments, &cronjobs, &jobs),
        vec!["deployment/shop-web", "cronjob/shop-nightly"]
    );
    assert_eq!(
        service_account_users("shop-db-bootstrap", &deployments, &cronjobs, &jobs),
        vec!["job/shop-db-bootstrap-1a2b"]
    );
    // A pod template without a ServiceAccount runs as `default`.
    assert_eq!(
        service_account_users("default", &deployments, &cronjobs, &jobs),
        vec!["deployment/plain"]
    );
    assert!(service_account_users("unused-sa", &deployments, &cronjobs, &jobs).is_empty());
}

// ── Against a fake API server ────────────────────────────────────────────────
//
// Serves the LISTs the in-use check makes and DELETEs of jobs and
// serviceaccounts. A DELETE of an object in `existing` succeeds (a deleted
// Job also disappears from the Job list, as with background propagation);
// one in `failing` gets a 500; anything else is a 404. Every request is
// recorded.

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    body: serde_json::Value,
}

#[derive(Default)]
struct FakeApiState {
    requests: Vec<RecordedRequest>,
    existing: BTreeSet<String>,
    failing: BTreeSet<String>,
    deployments: Vec<serde_json::Value>,
    jobs: Vec<serde_json::Value>,
    fail_lists: bool,
}

#[derive(Clone, Default)]
struct FakeApi(std::sync::Arc<std::sync::Mutex<FakeApiState>>);

const SA_PATH: &str = "/api/v1/namespaces/team/serviceaccounts";
const JOB_PATH: &str = "/apis/batch/v1/namespaces/team/jobs";

fn status(code: u16, reason: &str) -> (u16, serde_json::Value) {
    (
        code,
        serde_json::json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {},
            "status": "Failure", "message": reason, "reason": reason, "code": code,
        }),
    )
}

impl FakeApi {
    fn with(f: impl FnOnce(&mut FakeApiState)) -> Self {
        let api = FakeApi::default();
        f(&mut api.0.lock().unwrap());
        api
    }

    fn client(&self) -> kube::Client {
        let api = self.clone();
        let service = tower::service_fn(move |req: axum::http::Request<kube::client::Body>| {
            let api = api.clone();
            async move {
                let (parts, body) = req.into_parts();
                let bytes = body.collect_bytes().await.unwrap_or_default();
                let (code, json) = api.handle(&parts, &bytes);
                let response = axum::http::Response::builder()
                    .status(code)
                    .header("content-type", "application/json")
                    .body(kube::client::Body::from(serde_json::to_vec(&json).unwrap()))
                    .unwrap();
                Ok::<_, std::convert::Infallible>(response)
            }
        });
        kube::Client::new(service, "default")
    }

    fn handle(&self, parts: &axum::http::request::Parts, body: &[u8]) -> (u16, serde_json::Value) {
        let path = parts.uri.path().to_string();
        let mut st = self.0.lock().unwrap();
        st.requests.push(RecordedRequest {
            method: parts.method.to_string(),
            path: path.clone(),
            body: serde_json::from_slice(body).unwrap_or(serde_json::Value::Null),
        });
        let list = |kind: &str, items: Vec<serde_json::Value>| {
            (
                200,
                serde_json::json!({
                    "apiVersion": "v1", "kind": format!("{kind}List"),
                    "metadata": { "resourceVersion": "1" }, "items": items,
                }),
            )
        };
        match (parts.method.as_str(), path.as_str()) {
            ("GET", _) if st.fail_lists => status(500, "InternalError"),
            ("GET", "/apis/apps/v1/namespaces/team/deployments") => {
                list("Deployment", st.deployments.clone())
            }
            ("GET", "/apis/batch/v1/namespaces/team/cronjobs") => list("CronJob", vec![]),
            ("GET", JOB_PATH) => list("Job", st.jobs.clone()),
            ("DELETE", p) if st.failing.contains(p) => status(500, "InternalError"),
            ("DELETE", p) if st.existing.remove(p) => {
                let name = p.rsplit('/').next().unwrap_or_default().to_string();
                st.jobs.retain(|j| j["metadata"]["name"] != name.as_str());
                let (api_version, kind) = if p.starts_with(JOB_PATH) {
                    ("batch/v1", "Job")
                } else {
                    ("v1", "ServiceAccount")
                };
                (
                    200,
                    serde_json::json!({
                        "apiVersion": api_version, "kind": kind,
                        "metadata": { "name": name, "namespace": "team" },
                    }),
                )
            }
            ("DELETE", _) => status(404, "NotFound"),
            _ => status(405, "MethodNotAllowed"),
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.0.lock().unwrap().requests.clone()
    }

    fn deletes(&self) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "DELETE")
            .collect()
    }
}

async fn test_state(api: &FakeApi) -> AppState {
    let db = crate::db::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    AppState {
        kube_client: api.client(),
        allowed_namespaces: vec![],
        settings_namespace: "deckwatch".to_string(),
        settings_configmap_name: "deckwatch-settings".to_string(),
        entitlements: std::sync::Arc::new(crate::license::Entitlements::community()),
        registry_public_url: None,
        registry_internal_url: None,
        registry_enabled: false,
        ai_rate_limiter: crate::rate_limit::RateLimiter::default(),
        db,
        encryption_key: String::new(),
        plugins: std::sync::Arc::new(tokio::sync::RwLock::new(vec![])),
        plugin_patch_fingerprints: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
    }
}

async fn audit_entries(state: &AppState) -> Vec<audit_log::Model> {
    audit_log::Entity::find().all(&state.db).await.unwrap()
}

#[tokio::test]
async fn deletes_jobs_then_service_accounts_in_the_application_namespace() {
    let api = FakeApi::with(|st| {
        st.existing
            .insert(format!("{JOB_PATH}/shop-db-bootstrap-1a2b"));
        st.existing.insert(format!("{SA_PATH}/shop-db-bootstrap"));
        st.existing.insert(format!("{SA_PATH}/shop-aws-sa"));
        // The bootstrap Job still uses its ServiceAccount until it's deleted.
        st.jobs = vec![job_json("shop-db-bootstrap-1a2b", "shop-db-bootstrap")];
    });
    let state = test_state(&api).await;

    // ServiceAccounts listed before the Job on purpose.
    let requested = vec![
        obj("v1", "ServiceAccount", "shop-db-bootstrap"),
        obj("v1", "ServiceAccount", "shop-aws-sa"),
        obj("batch/v1", "Job", "shop-db-bootstrap-1a2b"),
    ];
    let deletions = delete_requested_objects(&state, &TARGET, &requested).await;

    let summary: Vec<(&str, DeletionOutcome)> = deletions
        .iter()
        .map(|d| (d.name.as_str(), d.outcome))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("shop-db-bootstrap-1a2b", DeletionOutcome::Deleted),
            ("shop-db-bootstrap", DeletionOutcome::Deleted),
            ("shop-aws-sa", DeletionOutcome::Deleted),
        ],
        "{deletions:#?}"
    );
    assert!(deletions.iter().all(|d| d.namespace == "team"));

    let deletes = api.deletes();
    let paths: Vec<&str> = deletes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/apis/batch/v1/namespaces/team/jobs/shop-db-bootstrap-1a2b",
            "/api/v1/namespaces/team/serviceaccounts/shop-db-bootstrap",
            "/api/v1/namespaces/team/serviceaccounts/shop-aws-sa",
        ]
    );
    for d in &deletes {
        assert_eq!(d.body["propagationPolicy"], "Background", "{d:?}");
    }
    // Never another namespace, for the DELETEs or the in-use LISTs.
    assert!(api
        .requests()
        .iter()
        .all(|r| r.path.contains("/namespaces/team/")));

    let audit = audit_entries(&state).await;
    assert_eq!(audit.len(), 3);
    for entry in &audit {
        assert_eq!(entry.action, "delete");
        assert_eq!(entry.namespace, "team");
        assert!(entry.detail.starts_with("deleted (requested by plugin 'AWS' deprovisioning resource 'rds' of application 'shop')"), "{}", entry.detail);
    }
    let types: BTreeSet<&str> = audit.iter().map(|e| e.resource_type.as_str()).collect();
    assert_eq!(types, BTreeSet::from(["job", "serviceaccount"]));
}

#[tokio::test]
async fn object_that_is_already_gone_counts_as_deleted() {
    let api = FakeApi::default();
    let state = test_state(&api).await;

    let deletions = delete_requested_objects(
        &state,
        &TARGET,
        &[
            obj("batch/v1", "Job", "shop-db-bootstrap-1a2b"),
            obj("v1", "ServiceAccount", "shop-aws-sa"),
        ],
    )
    .await;

    assert!(deletions
        .iter()
        .all(|d| d.outcome == DeletionOutcome::NotFound && d.reason.is_none()));
    assert_eq!(api.deletes().len(), 2);
    let audit = audit_entries(&state).await;
    assert!(audit
        .iter()
        .all(|e| e.detail.starts_with("already deleted")));
}

#[tokio::test]
async fn refuses_kinds_outside_the_allowlist_and_invalid_names() {
    let api = FakeApi::default();
    let state = test_state(&api).await;

    let requested = vec![
        obj("v1", "Secret", "shop-db-credentials"),
        obj("apps/v1", "Deployment", "shop-web"),
        obj("batch/v1beta1", "Job", "shop-db-bootstrap"),
        obj("v1", "serviceaccount", "shop-aws-sa"),
        obj(
            "v1",
            "ServiceAccount",
            "../../kube-system/serviceaccounts/x",
        ),
        obj("batch/v1", "Job", "x/y"),
        obj("v1", "ServiceAccount", "Shop-SA"),
        obj("v1", "ServiceAccount", ""),
        obj("", "", ""),
    ];
    let deletions = delete_requested_objects(&state, &TARGET, &requested).await;

    assert_eq!(deletions.len(), requested.len());
    assert!(
        deletions
            .iter()
            .all(|d| d.outcome == DeletionOutcome::Refused && d.reason.is_some()),
        "{deletions:#?}"
    );
    assert!(api.requests().is_empty(), "{:#?}", api.requests());

    let audit = audit_entries(&state).await;
    assert_eq!(audit.len(), requested.len());
    assert!(audit.iter().all(|e| e.detail.starts_with("refused: ")));
    assert!(audit.iter().any(|e| e.resource_type == "secret"));
}

#[tokio::test]
async fn keeps_a_service_account_that_workloads_still_use() {
    let api = FakeApi::with(|st| {
        st.existing.insert(format!("{SA_PATH}/shop-aws-sa"));
        st.deployments =
            vec![serde_json::to_value(deployment("shop-web", Some("shop-aws-sa"))).unwrap()];
    });
    let state = test_state(&api).await;

    let deletions = delete_requested_objects(
        &state,
        &TARGET,
        &[obj("v1", "ServiceAccount", "shop-aws-sa")],
    )
    .await;

    assert_eq!(deletions[0].outcome, DeletionOutcome::Refused);
    let reason = deletions[0].reason.as_deref().unwrap();
    assert!(reason.contains("deployment/shop-web"), "{reason}");
    assert!(api.deletes().is_empty());
    let audit = audit_entries(&state).await;
    assert!(
        audit[0]
            .detail
            .starts_with("refused: still used by deployment/shop-web"),
        "{}",
        audit[0].detail
    );
}

#[tokio::test]
async fn never_deletes_the_default_service_account() {
    let api = FakeApi::with(|st| {
        st.existing.insert(format!("{SA_PATH}/default"));
    });
    let state = test_state(&api).await;

    let deletions =
        delete_requested_objects(&state, &TARGET, &[obj("v1", "ServiceAccount", "default")]).await;

    assert_eq!(deletions[0].outcome, DeletionOutcome::Refused);
    assert!(api.requests().is_empty());
}

#[tokio::test]
async fn api_failures_are_reported_and_audited() {
    let api = FakeApi::with(|st| {
        st.failing
            .insert(format!("{JOB_PATH}/shop-db-bootstrap-1a2b"));
    });
    let state = test_state(&api).await;

    let deletions = delete_requested_objects(
        &state,
        &TARGET,
        &[obj("batch/v1", "Job", "shop-db-bootstrap-1a2b")],
    )
    .await;

    assert_eq!(deletions[0].outcome, DeletionOutcome::Failed);
    assert!(deletions[0].reason.is_some());
    let audit = audit_entries(&state).await;
    assert!(
        audit[0].detail.starts_with("failed: "),
        "{}",
        audit[0].detail
    );
    assert_eq!(audit[0].resource_type, "job");
    assert_eq!(audit[0].resource_name, "shop-db-bootstrap-1a2b");
}

#[tokio::test]
async fn service_account_is_kept_when_its_use_cannot_be_checked() {
    let api = FakeApi::with(|st| {
        st.existing.insert(format!("{SA_PATH}/shop-aws-sa"));
        st.fail_lists = true;
    });
    let state = test_state(&api).await;

    let deletions = delete_requested_objects(
        &state,
        &TARGET,
        &[obj("v1", "ServiceAccount", "shop-aws-sa")],
    )
    .await;

    assert_eq!(deletions[0].outcome, DeletionOutcome::Failed);
    assert!(api.deletes().is_empty());
}

#[tokio::test]
async fn failed_deletions_do_not_block_removing_the_resource() {
    let api = FakeApi::with(|st| {
        st.failing
            .insert(format!("{JOB_PATH}/shop-db-bootstrap-1a2b"));
    });
    let state = test_state(&api).await;
    crate::db::ensure_application(&state.db, "team", "shop")
        .await
        .unwrap();
    let now = now_utc();
    let row = application_plugin_resources::ActiveModel {
        id: Set("row-1".to_string()),
        application_id: Set("team/shop".to_string()),
        plugin_name: Set("AWS".to_string()),
        resource_id: Set("rds".to_string()),
        fields: Set("{}".to_string()),
        state: Set("{}".to_string()),
        annotations: Set("{}".to_string()),
        sidecars: Set("[]".to_string()),
        init_containers: Set("[]".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&state.db)
    .await
    .unwrap();

    let requested = vec![
        obj("batch/v1", "Job", "shop-db-bootstrap-1a2b"),
        obj("v1", "Secret", "shop-db"),
    ];
    let deletions = finish_deprovision(&state, &TARGET, row, &requested)
        .await
        .expect("record removed despite failed deletions");

    let outcomes: Vec<DeletionOutcome> = deletions.iter().map(|d| d.outcome).collect();
    assert_eq!(
        outcomes,
        vec![DeletionOutcome::Failed, DeletionOutcome::Refused]
    );
    let remaining = application_plugin_resources::Entity::find()
        .all(&state.db)
        .await
        .unwrap();
    assert!(remaining.is_empty());
}

#[tokio::test]
async fn deprovision_without_a_loaded_plugin_deletes_nothing() {
    let api = FakeApi::default();
    let state = test_state(&api).await;
    crate::db::ensure_application(&state.db, "team", "shop")
        .await
        .unwrap();
    let now = now_utc();
    application_plugin_resources::ActiveModel {
        id: Set("row-1".to_string()),
        application_id: Set("team/shop".to_string()),
        plugin_name: Set("AWS".to_string()),
        resource_id: Set("rds".to_string()),
        fields: Set("{}".to_string()),
        state: Set("{}".to_string()),
        annotations: Set("{}".to_string()),
        sidecars: Set("[]".to_string()),
        init_containers: Set("[]".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&state.db)
    .await
    .unwrap();

    let deletions = deprovision_resource(&state, &TARGET).await.unwrap();

    assert!(deletions.is_empty());
    assert!(api.requests().is_empty());
    assert!(application_plugin_resources::Entity::find()
        .all(&state.db)
        .await
        .unwrap()
        .is_empty());
}

// ── Live cluster ─────────────────────────────────────────────────────────────
//
// `#[ignore]`d like `integration_tests.rs`: needs a reachable API server from
// `KUBECONFIG` / `~/.kube/config`. Point it at a scratch cluster:
//
//   cargo test live_cluster_deprovision -- --ignored

const LIVE_NAMESPACE: &str = "deckwatch-it-deprovision";

async fn wait_until<F, Fut>(what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..90 {
        if done().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
#[ignore]
async fn live_cluster_deprovision_deletes_requested_objects() {
    use k8s_openapi::api::core::v1::{Namespace, Pod, ServiceAccount};
    use kube::api::PostParams;
    use kube::Api;

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = kube::Client::try_default()
        .await
        .expect("kube client -- is KUBECONFIG pointing at a scratch cluster?");
    let ns_api: Api<Namespace> = Api::all(client.clone());
    if ns_api.get_opt(LIVE_NAMESPACE).await.unwrap().is_none() {
        let ns: Namespace = serde_json::from_value(serde_json::json!({
            "metadata": { "name": LIVE_NAMESPACE }
        }))
        .unwrap();
        ns_api.create(&PostParams::default(), &ns).await.unwrap();
    }
    let sas: Api<ServiceAccount> = Api::namespaced(client.clone(), LIVE_NAMESPACE);
    let jobs: Api<Job> = Api::namespaced(client.clone(), LIVE_NAMESPACE);
    let deps: Api<Deployment> = Api::namespaced(client.clone(), LIVE_NAMESPACE);
    let pods: Api<Pod> = Api::namespaced(client.clone(), LIVE_NAMESPACE);
    let job_pods = ListParams::default().labels("job-name=shop-db-bootstrap-1a2b");

    // Leftovers from an earlier run, including pods a failed run orphaned.
    let _ = jobs
        .delete("shop-db-bootstrap-1a2b", &DeleteParams::background())
        .await;
    let _ = pods
        .delete_collection(&DeleteParams::default(), &job_pods)
        .await;
    let _ = deps.delete("shop-web", &DeleteParams::default()).await;
    for sa in ["shop-db-bootstrap", "shop-aws-sa"] {
        let _ = sas.delete(sa, &DeleteParams::default()).await;
    }
    wait_until("leftovers to go", || async {
        jobs.get_opt("shop-db-bootstrap-1a2b")
            .await
            .unwrap()
            .is_none()
            && deps.get_opt("shop-web").await.unwrap().is_none()
            && sas.get_opt("shop-db-bootstrap").await.unwrap().is_none()
            && sas.get_opt("shop-aws-sa").await.unwrap().is_none()
            && pods.list(&job_pods).await.unwrap().items.is_empty()
    })
    .await;

    for sa in ["shop-db-bootstrap", "shop-aws-sa"] {
        let sa: ServiceAccount =
            serde_json::from_value(serde_json::json!({ "metadata": { "name": sa } })).unwrap();
        sas.create(&PostParams::default(), &sa).await.unwrap();
    }
    let mut web = deployment("shop-web", Some("shop-aws-sa"));
    web.metadata.namespace = None;
    web.spec.as_mut().unwrap().replicas = Some(0);
    web.spec.as_mut().unwrap().template.metadata = Some(ObjectMeta {
        labels: Some([("app".to_string(), "shop-web".to_string())].into()),
        ..Default::default()
    });
    deps.create(&PostParams::default(), &web).await.unwrap();
    // The Job's pod never starts (the image doesn't exist), but it exists and
    // runs as the bootstrap ServiceAccount.
    let mut bootstrap = job_json("shop-db-bootstrap-1a2b", "shop-db-bootstrap");
    bootstrap["metadata"]["namespace"] = serde_json::json!(LIVE_NAMESPACE);
    bootstrap["spec"]["backoffLimit"] = serde_json::json!(0);
    bootstrap["spec"]["template"]["spec"]["containers"][0]["image"] =
        serde_json::json!("registry.invalid/psql:1");
    let bootstrap: Job = serde_json::from_value(bootstrap).unwrap();
    jobs.create(&PostParams::default(), &bootstrap)
        .await
        .unwrap();
    wait_until("the bootstrap Job's pod", || async {
        !pods.list(&job_pods).await.unwrap().items.is_empty()
    })
    .await;

    let state = AppState {
        kube_client: client.clone(),
        allowed_namespaces: vec![LIVE_NAMESPACE.to_string()],
        ..test_state(&FakeApi::default()).await
    };
    let target = DeprovisionTarget {
        namespace: LIVE_NAMESPACE,
        ..TARGET
    };
    let deletions = delete_requested_objects(
        &state,
        &target,
        &[
            obj("v1", "ServiceAccount", "shop-db-bootstrap"),
            obj("v1", "ServiceAccount", "shop-aws-sa"),
            obj("batch/v1", "Job", "shop-db-bootstrap-1a2b"),
            obj("v1", "ServiceAccount", "already-gone"),
            obj("v1", "Secret", "shop-db"),
        ],
    )
    .await;

    let outcomes: Vec<(&str, DeletionOutcome)> = deletions
        .iter()
        .map(|d| (d.name.as_str(), d.outcome))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            ("shop-db-bootstrap-1a2b", DeletionOutcome::Deleted),
            ("shop-db-bootstrap", DeletionOutcome::Deleted),
            ("shop-aws-sa", DeletionOutcome::Refused),
            ("already-gone", DeletionOutcome::NotFound),
            ("shop-db", DeletionOutcome::Refused),
        ],
        "{deletions:#?}"
    );
    assert!(jobs
        .get_opt("shop-db-bootstrap-1a2b")
        .await
        .unwrap()
        .is_none());
    assert!(sas.get_opt("shop-db-bootstrap").await.unwrap().is_none());
    assert!(sas.get_opt("shop-aws-sa").await.unwrap().is_some());
    // Background propagation: the garbage collector deletes the Job's pods.
    wait_until("the bootstrap Job's pods to be deleted", || async {
        pods.list(&job_pods).await.unwrap().items.is_empty()
    })
    .await;

    let _ = deps.delete("shop-web", &DeleteParams::default()).await;
    let _ = sas.delete("shop-aws-sa", &DeleteParams::default()).await;
}
