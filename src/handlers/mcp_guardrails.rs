//! The guardrails of the embedded mcp-k8s tools as deckwatch settings
//! (Settings → MCP Tuning → Kubernetes tool guardrails,
//! `mcp_tuning.guardrails`), and the `ActionPermissions` they build for
//! every MCP request.
//!
//! mcp-k8s checks its typed write tools and its custom resource tools against
//! a workload policy (since 0.6), can serve `apply_manifest`, a raw
//! server-side apply that skips that policy, and returns Secret values only
//! when allowed to (since 0.7). Deckwatch's defaults:
//!
//! - the restrictive workload policy, with no exceptions and no custom
//!   resource API groups (so custom resource writes are refused);
//! - `apply_manifest` on: it still bypasses the policy, and its `destructiveHint`
//!   makes MCP clients ask the user before each call;
//! - Secret values off;
//! - approving CSRs for privileged identities off;
//! - secure defaults on (the typed create tools fill in a restricted
//!   securityContext).
//!
//! `tools/list` and `tools/call` read the settings on every request, so a
//! change applies to the next call without a restart.

use std::collections::HashSet;
use std::sync::OnceLock;

use mcp_k8s::permissions::ActionPermissions;
use mcp_k8s::policy::{flags, PolicyFlag, WorkloadPolicy};
use serde::{Deserialize, Serialize};

/// Where an admin changes these settings, for messages.
pub const SETTINGS_PATH: &str = "Settings → MCP Tuning → Kubernetes tool guardrails";

/// How strict the workload policy is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadPolicyMode {
    /// mcp-k8s's `WorkloadPolicy::default()`, relaxed only by the
    /// [`McpGuardrails::exceptions`] and the custom resource API groups.
    #[default]
    Restrictive,
    /// `WorkloadPolicy::permissive()`: every workload guardrail off (mcp-k8s
    /// 0.5 behaviour). The CSR check and the secure defaults keep their own
    /// switches.
    Permissive,
}

impl WorkloadPolicyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restrictive => "restrictive",
            Self::Permissive => "permissive",
        }
    }
}

/// The mcp-k8s guardrail settings (`mcp_tuning.guardrails`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpGuardrails {
    /// `restrictive` (default) or `permissive`.
    #[serde(default)]
    pub workload_policy: WorkloadPolicyMode,
    /// Rules of the restrictive policy to relax, by their mcp-k8s
    /// `WorkloadPolicy` field name ([`EXCEPTIONS`]), e.g.
    /// `allow_load_balancer`. Ignored in permissive mode.
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// API groups whose custom resources `create_custom_resource` and
    /// `update_custom_resource` may write (`"*"`: any group). Empty, the
    /// default, refuses custom resource writes: operators can turn custom
    /// resources into pods or RBAC objects. Ignored in permissive mode,
    /// which allows every group.
    #[serde(default)]
    pub allowed_custom_resource_groups: Vec<String>,
    /// Let `approve_csr` approve CertificateSigningRequests for privileged
    /// identities (`system:masters`, any other `system:` user or group).
    /// Such a certificate can be cluster-admin. Off by default, in either
    /// mode.
    #[serde(default)]
    pub allow_privileged_csr: bool,
    /// List and serve `apply_manifest`. On by default. It bypasses the
    /// workload policy; MCP clients ask the user before each call.
    #[serde(default = "enabled")]
    pub apply_manifest_enabled: bool,
    /// Let `get_secret` return Secret values (`decode: true`), and
    /// `get_resource_json` return a Secret's `data`. Off by default.
    #[serde(default)]
    pub secret_decode_enabled: bool,
    /// The typed create tools fill in `runAsNonRoot: true`, seccomp
    /// `RuntimeDefault`, `allowPrivilegeEscalation: false`, dropping all
    /// capabilities and `automountServiceAccountToken: false` when the
    /// caller doesn't set them. On by default, in either mode.
    #[serde(default = "enabled")]
    pub secure_defaults: bool,
}

fn enabled() -> bool {
    true
}

impl Default for McpGuardrails {
    fn default() -> Self {
        Self {
            workload_policy: WorkloadPolicyMode::Restrictive,
            exceptions: Vec::new(),
            allowed_custom_resource_groups: Vec::new(),
            allow_privileged_csr: false,
            apply_manifest_enabled: true,
            secret_decode_enabled: false,
            secure_defaults: true,
        }
    }
}

/// One rule of the restrictive policy that [`McpGuardrails::exceptions`] can
/// relax.
pub struct Exception {
    /// The mcp-k8s setting; its `field` is the name used in `exceptions`.
    pub flag: &'static PolicyFlag,
    relax: fn(&mut WorkloadPolicy),
}

/// Every boolean rule of mcp-k8s's `WorkloadPolicy` except
/// `allow_privileged_csr`, which has its own setting. A test fails when
/// mcp-k8s adds a rule that is neither here nor deliberately left out.
pub const EXCEPTIONS: &[Exception] = &[
    Exception {
        flag: &flags::ALLOW_PRIVILEGED,
        // Allows privileged containers, and namespaces labelled with the
        // privileged Pod Security level.
        relax: |p| p.allow_privileged = true,
    },
    Exception {
        flag: &flags::ALLOW_PRIVILEGE_ESCALATION,
        // Allows allowPrivilegeEscalation: true.
        relax: |p| p.allow_privilege_escalation = true,
    },
    Exception {
        flag: &flags::ALLOW_HOST_NAMESPACES,
        // Allows hostNetwork, hostPID and hostIPC.
        relax: |p| p.allow_host_namespaces = true,
    },
    Exception {
        flag: &flags::ALLOW_HOST_PATH,
        // Allows hostPath volumes.
        relax: |p| p.allow_host_path = true,
    },
    Exception {
        flag: &flags::ALLOW_HOST_PORTS,
        // Allows hostPort on container ports.
        relax: |p| p.allow_host_ports = true,
    },
    Exception {
        flag: &flags::ALLOW_UNCONFINED,
        // Allows Unconfined seccomp or AppArmor, custom SELinux options, unsafe sysctls.
        relax: |p| p.allow_unconfined = true,
    },
    Exception {
        flag: &flags::ALLOW_NODEPORT,
        // Allows NodePort Services.
        relax: |p| p.allow_node_port = true,
    },
    Exception {
        flag: &flags::ALLOW_LOADBALANCER,
        // Allows LoadBalancer Services.
        relax: |p| p.allow_load_balancer = true,
    },
    Exception {
        flag: &flags::ALLOW_EXTERNAL_SERVICES,
        // Allows ExternalName Services and externalIPs.
        relax: |p| p.allow_external_services = true,
    },
    Exception {
        flag: &flags::ALLOW_INGRESS_SNIPPETS,
        // Allows ingress-nginx snippet annotations and nginx configuration
        // in annotation values.
        relax: |p| p.allow_ingress_snippets = true,
    },
    Exception {
        flag: &flags::ALLOW_RBAC_ESCALATION,
        // Allows RBAC escalation: escalate, bind and impersonate verbs,
        // wildcards, aggregation, binding cluster-admin.
        relax: |p| p.allow_rbac_escalation = true,
    },
];

fn exception(field: &str) -> Option<&'static Exception> {
    EXCEPTIONS.iter().find(|e| e.flag.field == field)
}

impl McpGuardrails {
    /// The workload policy these settings give.
    pub fn workload_policy(&self) -> WorkloadPolicy {
        let mut policy = match self.workload_policy {
            WorkloadPolicyMode::Restrictive => {
                let mut policy = WorkloadPolicy::default();
                for name in &self.exceptions {
                    if let Some(e) = exception(name) {
                        (e.relax)(&mut policy);
                    }
                }
                policy
                    .allowed_custom_resource_groups
                    .clone_from(&self.allowed_custom_resource_groups);
                policy
            }
            WorkloadPolicyMode::Permissive => WorkloadPolicy::permissive(),
        };
        policy.allow_privileged_csr = self.allow_privileged_csr;
        policy.secure_defaults = self.secure_defaults;
        policy
    }

    /// The permissions every MCP call is checked with: all actions allowed
    /// (deckwatch has no read-only mode), with these guardrails.
    pub fn permissions(&self) -> ActionPermissions {
        ActionPermissions::default()
            .with_workload_policy(self.workload_policy())
            .with_apply_manifest(self.apply_manifest_enabled)
            .with_secret_decode(self.secret_decode_enabled)
    }

    /// A normalized copy, or why the settings are invalid: exceptions must
    /// be known rule names (kept once each, in [`EXCEPTIONS`] order), and
    /// custom resource API groups `*` or DNS subdomains that aren't a
    /// built-in group (trimmed, lowercased, kept once each).
    pub fn validated(mut self) -> Result<Self, String> {
        let unknown: Vec<&str> = self
            .exceptions
            .iter()
            .map(|e| e.trim())
            .filter(|e| exception(e).is_none())
            .collect();
        if !unknown.is_empty() {
            let known: Vec<&str> = EXCEPTIONS.iter().map(|e| e.flag.field).collect();
            let csr = if unknown.contains(&flags::ALLOW_PRIVILEGED_CSR.field) {
                " (allow_privileged_csr is a setting of its own)"
            } else {
                ""
            };
            return Err(format!(
                "mcp_tuning.guardrails.exceptions: unknown rule {}{csr}; the rules are {}",
                unknown.join(", "),
                known.join(", ")
            ));
        }
        self.exceptions = EXCEPTIONS
            .iter()
            .map(|e| e.flag.field)
            .filter(|field| self.exceptions.iter().any(|x| x.trim() == *field))
            .map(str::to_string)
            .collect();

        let mut groups: Vec<String> = Vec::new();
        for group in &self.allowed_custom_resource_groups {
            let group = group.trim().to_ascii_lowercase();
            if group.is_empty() || groups.contains(&group) {
                continue;
            }
            if group != "*" {
                check_api_group(&group)?;
            }
            groups.push(group);
        }
        self.allowed_custom_resource_groups = groups;
        Ok(self)
    }

    /// The settings that differ between `before` and `after`, for the audit
    /// log.
    pub fn changes(before: &Self, after: &Self) -> Vec<GuardrailChange> {
        let list = |v: &[String]| {
            if v.is_empty() {
                "none".to_string()
            } else {
                v.join(", ")
            }
        };
        let fields: [(&'static str, String, String); 7] = [
            (
                "workload_policy",
                before.workload_policy.as_str().to_string(),
                after.workload_policy.as_str().to_string(),
            ),
            (
                "exceptions",
                list(&before.exceptions),
                list(&after.exceptions),
            ),
            (
                "allowed_custom_resource_groups",
                list(&before.allowed_custom_resource_groups),
                list(&after.allowed_custom_resource_groups),
            ),
            (
                "allow_privileged_csr",
                before.allow_privileged_csr.to_string(),
                after.allow_privileged_csr.to_string(),
            ),
            (
                "apply_manifest_enabled",
                before.apply_manifest_enabled.to_string(),
                after.apply_manifest_enabled.to_string(),
            ),
            (
                "secret_decode_enabled",
                before.secret_decode_enabled.to_string(),
                after.secret_decode_enabled.to_string(),
            ),
            (
                "secure_defaults",
                before.secure_defaults.to_string(),
                after.secure_defaults.to_string(),
            ),
        ];
        fields
            .into_iter()
            .filter(|(_, from, to)| from != to)
            .map(|(field, from, to)| GuardrailChange { field, from, to })
            .collect()
    }
}

/// `group` is a DNS subdomain and not a built-in API group.
fn check_api_group(group: &str) -> Result<(), String> {
    let label_ok = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if group.len() > 253 || !group.split('.').all(label_ok) {
        return Err(format!(
            "mcp_tuning.guardrails.allowed_custom_resource_groups: {group:?} is not an API group \
             name (a DNS subdomain such as cert-manager.io), or \"*\" for any group"
        ));
    }
    if mcp_k8s::policy::is_builtin_group(group) {
        return Err(format!(
            "mcp_tuning.guardrails.allowed_custom_resource_groups: {group} is a built-in API \
             group; the custom resource tools refuse built-in kinds, the typed tools write them"
        ));
    }
    Ok(())
}

/// One guardrail setting changed by a settings PUT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardrailChange {
    /// The field of `mcp_tuning.guardrails`.
    pub field: &'static str,
    pub from: String,
    pub to: String,
}

impl GuardrailChange {
    /// The setting's path, the audit row's resource name.
    pub fn setting(&self) -> String {
        format!("mcp_tuning.guardrails.{}", self.field)
    }

    /// Audit-log text.
    pub fn describe(&self) -> String {
        format!(
            "{} changed from {} to {}; MCP tool calls use it from the next request",
            self.setting(),
            self.from,
            self.to
        )
    }
}

// ---------------------------------------------------------------------------
// Tools deckwatch doesn't serve
// ---------------------------------------------------------------------------

/// Whether `tool` is an mcp-k8s tool that deckwatch doesn't serve, so
/// `tools/list` mustn't offer it: the multi-cluster tools (`list_clusters`,
/// `switch_cluster`, `get_active_cluster`). Only the standalone mcp-k8s
/// server answers them (it holds the cluster contexts); `handle_tool` doesn't,
/// and a deckwatch instance manages the one cluster it runs in.
pub fn is_unserved(tool: &str) -> bool {
    static UNSERVED: OnceLock<HashSet<String>> = OnceLock::new();
    UNSERVED
        .get_or_init(|| {
            mcp_k8s::resources::cluster_mgmt::tool_definitions()
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
                .collect()
        })
        .contains(tool)
}

// ---------------------------------------------------------------------------
// Refusal messages
// ---------------------------------------------------------------------------

/// What a call of `apply_manifest` gets while it is turned off.
pub fn apply_manifest_disabled() -> String {
    format!(
        "apply_manifest is turned off on this deckwatch ({SETTINGS_PATH} → apply_manifest, \
         mcp_tuning.guardrails.apply_manifest_enabled). Use the typed create_*/update_* tools. \
         Don't ask for it to be turned on to get around a rejection."
    )
}

/// The start of mcp-k8s's workload policy rejections (`PolicyError`).
const POLICY_REJECTION: &str = "rejected by the mcp-k8s workload policy";
/// How a policy rejection names each setting that would allow it.
const POLICY_FIELD: &str = "library: WorkloadPolicy.";
/// mcp-k8s's refusal of `get_secret` with `decode: true`.
const SECRET_DECODE_REFUSAL: &str = "secret decoding is disabled on this server";

/// `error` with the deckwatch settings that would allow it, when it is an
/// mcp-k8s refusal. mcp-k8s names its own server's flags and environment
/// variables (`--allow-loadbalancer`, `--enable-secret-decode`), which a
/// deckwatch admin can't set; the hint names the deckwatch setting instead.
pub fn with_setting_hint(error: String) -> String {
    match setting_hint(&error) {
        Some(hint) => format!("{error}\n{hint}"),
        None => error,
    }
}

/// The deckwatch settings that would allow a call mcp-k8s refused with
/// `error`, or `None` if it isn't such a refusal.
pub fn setting_hint(error: &str) -> Option<String> {
    if error.contains(SECRET_DECODE_REFUSAL) {
        return Some(format!(
            "On deckwatch, an admin turns Secret values on in {SETTINGS_PATH} → Return Secret \
             values (mcp_tuning.guardrails.secret_decode_enabled), not with \
             --enable-secret-decode."
        ));
    }
    if !error.contains(POLICY_REJECTION) {
        return None;
    }
    let mut fields: Vec<&str> = Vec::new();
    let mut rest = error;
    while let Some(at) = rest.find(POLICY_FIELD) {
        rest = &rest[at + POLICY_FIELD.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let field = &rest[..end];
        if !field.is_empty() && !fields.contains(&field) {
            fields.push(field);
        }
    }
    let remedies: Vec<String> = fields.iter().map(|field| remedy(field)).collect();
    if remedies.is_empty() {
        return None;
    }
    Some(format!(
        "On deckwatch, an admin sets these guardrails in {SETTINGS_PATH} \
         (mcp_tuning.guardrails), not with the mcp-k8s server flags above: {}.",
        remedies.join("; ")
    ))
}

/// How a deckwatch admin allows what WorkloadPolicy field `field` forbids.
fn remedy(field: &str) -> String {
    if exception(field).is_some() {
        format!("{field} is an exception to add (mcp_tuning.guardrails.exceptions)")
    } else if field == flags::ALLOW_CUSTOM_RESOURCE_GROUPS.field {
        "the API group goes in the allowed custom resource API groups \
         (mcp_tuning.guardrails.allowed_custom_resource_groups)"
            .to_string()
    } else if field == flags::ALLOW_PRIVILEGED_CSR.field {
        "approving privileged CSRs is a switch of its own \
         (mcp_tuning.guardrails.allow_privileged_csr)"
            .to_string()
    } else {
        format!(
            "{field} isn't a deckwatch setting; only the permissive policy allows it \
             (mcp_tuning.guardrails.workload_policy)"
        )
    }
}

#[cfg(test)]
#[path = "mcp_guardrails_tests.rs"]
mod tests;
