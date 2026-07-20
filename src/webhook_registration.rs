#![allow(dead_code, unused_imports)]
//! Creates/updates the cluster-scoped `ValidatingWebhookConfiguration`
//! on startup so the API server knows to route Deployment and Ingress
//! admissions to our HTTPS listener.
//!
//! `caBundle` is injected from [`webhook_tls::WebhookTls`] — cert
//! rotation is implicit: a rolling restart mints a new CA and patches
//! this resource in one step. `failurePolicy: Ignore` is hard-coded in
//! v1 (design doc §5, §6.1); upgrading to `Fail` is a Phase 5 concern.
//!
//! The webhook config's `namespaceSelector` uses the
//! `allowed_namespaces` list on `AppState`. When the list is empty
//! (deckwatch is watching the whole cluster) we omit the selector so
//! the webhook applies globally except for the excluded system
//! namespaces baked into `PROTECTED_NAMESPACE_SELECTOR`.

use anyhow::{Context, Result};
use k8s_openapi::api::admissionregistration::v1::{
    RuleWithOperations, ServiceReference, ValidatingWebhook, ValidatingWebhookConfiguration,
    WebhookClientConfig,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{
    LabelSelector, LabelSelectorRequirement, ObjectMeta,
};
use kube::api::{Api, Patch, PatchParams};

/// Name of the ValidatingWebhookConfiguration resource. Kept as a
/// const so cleanup on uninstall can locate it deterministically.
pub const WEBHOOK_CONFIG_NAME: &str = "deckwatch";

/// Field manager used for server-side apply. Distinct from the
/// deckwatch-app manager string so `kubectl get ...
/// -o=jsonpath='{.metadata.managedFields}'` shows the webhook lifecycle
/// separately from the settings-CM write path.
const FIELD_MANAGER: &str = "deckwatch-webhook";

/// System namespaces we always exclude from admission — belt-and-braces
/// with the handler-side `PROTECTED_NAMESPACES` list. The API server
/// respects this at admission-time so a broken handler cannot dent
/// system components.
const PROTECTED_NAMESPACE_SELECTOR: &[&str] = &["kube-system", "kube-public", "kube-node-lease"];

/// Input to [`ensure`]: the webhook Service coordinates + CA bundle.
pub struct RegistrationInput<'a> {
    pub service_name: &'a str,
    pub service_namespace: &'a str,
    pub service_path: &'a str,
    pub ca_bundle_b64: &'a str,
    /// Namespaces deckwatch is scoped to. Empty = all namespaces
    /// (`namespaceSelector` is left off).
    pub allowed_namespaces: &'a [String],
}

/// Server-side-apply the ValidatingWebhookConfiguration. Idempotent —
/// running this on every startup keeps the CA bundle fresh.
pub async fn ensure(client: &kube::Client, input: RegistrationInput<'_>) -> Result<()> {
    let api: Api<ValidatingWebhookConfiguration> = Api::all(client.clone());

    // caBundle needs to be raw bytes on the wire; k8s-openapi models
    // it as `Option<ByteString>` which serializes as base64 to JSON
    // already, so we hand it the *decoded* bytes.
    let ca_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        input.ca_bundle_b64,
    )
    .context("decoding caBundle base64")?;

    let namespace_selector = build_namespace_selector(input.allowed_namespaces);

    let deployment_webhook = ValidatingWebhook {
        name: "deployments.deckwatch.io".to_string(),
        admission_review_versions: vec!["v1".to_string()],
        side_effects: "None".to_string(),
        failure_policy: Some("Ignore".to_string()),
        timeout_seconds: Some(5),
        match_policy: Some("Equivalent".to_string()),
        namespace_selector: namespace_selector.clone(),
        rules: Some(vec![RuleWithOperations {
            api_groups: Some(vec!["apps".to_string()]),
            api_versions: Some(vec!["v1".to_string()]),
            resources: Some(vec!["deployments".to_string()]),
            operations: Some(vec!["CREATE".to_string(), "UPDATE".to_string()]),
            scope: Some("Namespaced".to_string()),
        }]),
        client_config: WebhookClientConfig {
            ca_bundle: Some(k8s_openapi::ByteString(ca_bytes.clone())),
            service: Some(ServiceReference {
                name: input.service_name.to_string(),
                namespace: input.service_namespace.to_string(),
                path: Some(input.service_path.to_string()),
                port: Some(443),
            }),
            url: None,
        },
        ..Default::default()
    };

    let ingress_webhook = ValidatingWebhook {
        name: "ingresses.deckwatch.io".to_string(),
        admission_review_versions: vec!["v1".to_string()],
        side_effects: "None".to_string(),
        failure_policy: Some("Ignore".to_string()),
        timeout_seconds: Some(5),
        match_policy: Some("Equivalent".to_string()),
        namespace_selector,
        rules: Some(vec![RuleWithOperations {
            api_groups: Some(vec!["networking.k8s.io".to_string()]),
            api_versions: Some(vec!["v1".to_string()]),
            resources: Some(vec!["ingresses".to_string()]),
            operations: Some(vec!["CREATE".to_string(), "UPDATE".to_string()]),
            scope: Some("Namespaced".to_string()),
        }]),
        client_config: WebhookClientConfig {
            ca_bundle: Some(k8s_openapi::ByteString(ca_bytes)),
            service: Some(ServiceReference {
                name: input.service_name.to_string(),
                namespace: input.service_namespace.to_string(),
                path: Some(input.service_path.to_string()),
                port: Some(443),
            }),
            url: None,
        },
        ..Default::default()
    };

    let cfg = ValidatingWebhookConfiguration {
        metadata: ObjectMeta {
            name: Some(WEBHOOK_CONFIG_NAME.to_string()),
            labels: Some(
                [
                    (
                        "app.kubernetes.io/managed-by".to_string(),
                        "deckwatch".to_string(),
                    ),
                    (
                        "app.kubernetes.io/component".to_string(),
                        "admission-webhook".to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        },
        webhooks: Some(vec![deployment_webhook, ingress_webhook]),
    };

    let params = PatchParams::apply(FIELD_MANAGER).force();
    api.patch(WEBHOOK_CONFIG_NAME, &params, &Patch::Apply(&cfg))
        .await
        .context("applying ValidatingWebhookConfiguration")?;

    tracing::info!(
        webhook = WEBHOOK_CONFIG_NAME,
        service = input.service_name,
        namespace = input.service_namespace,
        "ValidatingWebhookConfiguration registered",
    );
    Ok(())
}

/// Build the `namespaceSelector`:
///   * Always exclude the system namespaces in [`PROTECTED_NAMESPACE_SELECTOR`].
///   * If `allowed_namespaces` is non-empty, additionally require the
///     namespace to be in that list. When empty, only the exclusion
///     runs (deckwatch is watching everything).
fn build_namespace_selector(allowed: &[String]) -> Option<LabelSelector> {
    let mut expressions: Vec<LabelSelectorRequirement> = Vec::new();
    expressions.push(LabelSelectorRequirement {
        key: "kubernetes.io/metadata.name".to_string(),
        operator: "NotIn".to_string(),
        values: Some(
            PROTECTED_NAMESPACE_SELECTOR
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
    });
    if !allowed.is_empty() {
        expressions.push(LabelSelectorRequirement {
            key: "kubernetes.io/metadata.name".to_string(),
            operator: "In".to_string(),
            values: Some(allowed.to_vec()),
        });
    }
    Some(LabelSelector {
        match_expressions: Some(expressions),
        match_labels: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_excludes_system_ns_when_scope_is_cluster_wide() {
        let sel = build_namespace_selector(&[]).unwrap();
        let exprs = sel.match_expressions.as_ref().unwrap();
        assert_eq!(exprs.len(), 1);
        assert_eq!(exprs[0].operator, "NotIn");
        assert!(exprs[0]
            .values
            .as_ref()
            .unwrap()
            .iter()
            .any(|v| v == "kube-system"));
    }

    #[test]
    fn selector_adds_allowlist_when_scoped() {
        let allowed = vec!["ns-a".to_string(), "ns-b".to_string()];
        let sel = build_namespace_selector(&allowed).unwrap();
        let exprs = sel.match_expressions.as_ref().unwrap();
        assert_eq!(exprs.len(), 2);
        let in_expr = exprs.iter().find(|e| e.operator == "In").unwrap();
        let vals = in_expr.values.as_ref().unwrap();
        assert!(vals.contains(&"ns-a".to_string()));
    }
}
