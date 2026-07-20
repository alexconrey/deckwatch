#![allow(dead_code, unused_imports)]
//! `GET /api/license` — returns the current license status for the frontend
//! Settings page to render the tier chip, expiry banner, and feature matrix.
//!
//! This endpoint MUST be reachable without a Pro/Enterprise gate — the
//! Community-tier UI needs to render "Upgrade to Pro" affordances (per
//! `docs/LICENSING_STRATEGY.md` §2.4: "show them with an 'Upgrade'
//! affordance"), so the endpoint is public read-only.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::license::Limits;
use crate::state::AppState;

/// Response body for `GET /api/license`. Shaped to feed
/// `frontend/src/composables/useLicense.ts` directly with no reshaping.
#[derive(Serialize)]
pub struct LicenseStatus {
    pub tier: &'static str,
    /// RFC-3339 timestamp; `null` when there is no license (Community
    /// fallback with no origin).
    pub expires_at: Option<String>,
    /// True when the underlying license has passed `exp` but is still inside
    /// the 30-day grace window; the UI uses this to render a "renew soon"
    /// banner without changing feature availability.
    pub in_grace: bool,
    /// Sorted list of granted feature identifiers. Frontend maps this against
    /// its known feature catalog to render the paywall matrix.
    pub features: Vec<String>,
    /// Universe of features the frontend can display. Emitting this from the
    /// backend keeps the two sides from drifting when new tiers are added.
    pub feature_catalog: Vec<FeatureCatalogEntry>,
    pub limits: Limits,
    pub customer: Option<CustomerView>,
    pub license_id: Option<String>,
    /// Convenience: `expires_at - now` in whole seconds. Negative when in
    /// grace. Null when there is no expiry. Saves the frontend from doing
    /// clock arithmetic across timezones.
    pub seconds_until_expiry: Option<i64>,
}

#[derive(Serialize)]
pub struct CustomerView {
    pub org: String,
    pub contact: String,
}

#[derive(Serialize)]
pub struct FeatureCatalogEntry {
    pub feature: &'static str,
    pub tier: &'static str,
    pub label: &'static str,
    /// Short description shown in the Settings feature matrix.
    pub description: &'static str,
}

pub async fn get_license(State(state): State<AppState>) -> Json<LicenseStatus> {
    let ent = &state.entitlements;

    let expires_at = ent.expires_at().map(|ts| ts.to_string());
    let seconds_until_expiry = ent.expires_at().map(|exp| {
        let now = jiff::Timestamp::now();
        (exp.as_second()) - now.as_second()
    });

    let customer = ent.customer().map(|c| CustomerView {
        org: c.org.clone(),
        contact: c.contact.clone(),
    });

    Json(LicenseStatus {
        tier: ent.tier().as_str(),
        expires_at,
        in_grace: ent.in_grace(),
        features: ent.features(),
        feature_catalog: feature_catalog(),
        limits: ent.limits().clone(),
        customer,
        license_id: ent.license_id().map(|s| s.to_string()),
        seconds_until_expiry,
    })
}

/// Static catalog of all gate-able features across every tier. Kept in this
/// file (not in `license.rs`) because it's UI-facing labels — the tier logic
/// itself lives with the entitlements code.
fn feature_catalog() -> Vec<FeatureCatalogEntry> {
    vec![
        // Pro
        FeatureCatalogEntry {
            feature: "ai_diagnostics",
            tier: "pro",
            label: "AI Diagnostics",
            description: "LLM-driven root-cause hints on failing pods and deployments.",
        },
        FeatureCatalogEntry {
            feature: "ai_code_fix",
            tier: "pro",
            label: "AI Code Fix",
            description: "Proposes patched Deployment specs based on failure analysis.",
        },
        FeatureCatalogEntry {
            feature: "sso_multi_tenant",
            tier: "pro",
            label: "Enterprise SSO",
            description: "Multi-tenant OIDC, SAML, and SCIM provisioning.",
        },
        FeatureCatalogEntry {
            feature: "webhook_notifications",
            tier: "pro",
            label: "Webhook Notifications",
            description: "Slack, Teams, PagerDuty, and generic webhooks on cluster events.",
        },
        FeatureCatalogEntry {
            feature: "prometheus_charts",
            tier: "pro",
            label: "Prometheus Integration",
            description: "PromQL charts, historical windows, alerting rules.",
        },
        FeatureCatalogEntry {
            feature: "extended_log_search",
            tier: "pro",
            label: "Extended Log Search",
            description: "Search, filter, and download logs across multiple pods.",
        },
        FeatureCatalogEntry {
            feature: "audit_30d",
            tier: "pro",
            label: "Audit Trail (30 day)",
            description: "Namespace-scoped audit log of who did what, when.",
        },
        // Enterprise
        FeatureCatalogEntry {
            feature: "multi_cluster",
            tier: "enterprise",
            label: "Multi-Cluster Management",
            description: "Cluster switcher, cross-cluster search, per-cluster RBAC.",
        },
        FeatureCatalogEntry {
            feature: "audit_immutable",
            tier: "enterprise",
            label: "Immutable Audit (1yr)",
            description: "Tamper-evident audit storage with SIEM export.",
        },
        FeatureCatalogEntry {
            feature: "custom_branding",
            tier: "enterprise",
            label: "Custom Branding",
            description: "Logo, colors, product name, and login page customization.",
        },
        FeatureCatalogEntry {
            feature: "advanced_rbac",
            tier: "enterprise",
            label: "Advanced RBAC",
            description: "Custom roles and attribute-based access policies.",
        },
        FeatureCatalogEntry {
            feature: "compliance_packs",
            tier: "enterprise",
            label: "Compliance Packs",
            description: "CIS benchmark checks and SOC2 evidence exports.",
        },
        FeatureCatalogEntry {
            feature: "air_gapped_installer",
            tier: "enterprise",
            label: "Air-Gapped Installer",
            description: "Mirrored images and offline license activation.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{Entitlements, License, LicensePayload, Limits, Tier};

    #[test]
    fn catalog_covers_every_pro_and_enterprise_default_feature() {
        let catalog: std::collections::HashSet<&str> =
            feature_catalog().iter().map(|e| e.feature).collect();
        for f in Tier::Pro.default_features() {
            assert!(catalog.contains(f), "catalog missing Pro feature {f}");
        }
        for f in Tier::Enterprise.default_features() {
            assert!(
                catalog.contains(f),
                "catalog missing Enterprise feature {f}"
            );
        }
    }

    #[test]
    fn catalog_tier_matches_required_tier() {
        for entry in feature_catalog() {
            let expected = Entitlements::required_tier(entry.feature).as_str();
            assert_eq!(
                entry.tier, expected,
                "catalog entry for {} says {} but required_tier says {}",
                entry.feature, entry.tier, expected
            );
        }
    }

    #[test]
    fn community_license_has_no_expiry_no_features() {
        let ent = Entitlements::community();
        // Not calling the handler directly (needs AppState); just exercise
        // the shape of what get_license would serialize.
        assert_eq!(ent.tier().as_str(), "community");
        assert!(ent.expires_at().is_none());
        assert!(ent.features().is_empty());
    }

    #[test]
    fn pro_license_status_reports_expected_shape() {
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust".into(),
            iat: 1_700_000_000,
            exp: 9_999_999_999,
            jti: "lic-shape".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits {
                max_users: Some(25),
                max_clusters: Some(1),
            },
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let ent = Entitlements::from_license(
            &license,
            jiff::Timestamp::from_second(1_800_000_000).unwrap(),
        );
        assert_eq!(ent.tier(), Tier::Pro);
        assert_eq!(ent.limits().max_users, Some(25));
        assert!(ent.features().iter().any(|f| f == "ai_diagnostics"));
    }
}
