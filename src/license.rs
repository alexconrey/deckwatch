#![allow(dead_code, unused_imports)]
//! License enforcement runtime.
//!
//! Deckwatch licenses are Ed25519-signed JSON blobs. The Deckwatch binary
//! ships with a hard-coded Ed25519 public key (see [`EMBEDDED_PUBLIC_KEY`]);
//! at startup the runtime loads the raw signed blob from either the
//! `DECKWATCH_LICENSE_KEY` environment variable or a Kubernetes Secret
//! (`deckwatch-license/license.jwt`), verifies the signature, checks the
//! expiry with a 30-day grace period, and constructs the [`Entitlements`]
//! object that gates Pro/Enterprise features across the API.
//!
//! ## The pledge
//!
//! Community-tier features are NEVER gated. If license loading fails, if the
//! signature is invalid, if the license is expired past the grace period, if
//! DECKWATCH_LICENSE_KEY is absent — the app degrades to `Tier::Community`
//! and continues serving all cluster-control endpoints. This module MUST NOT
//! ever return an error path that hard-fails startup.
//!
//! See `docs/LICENSING_STRATEGY.md` for the full tier design and
//! `docs/LICENSE_RECOMMENDATION.md` for the OSS/commercial split rationale.

use std::collections::HashSet;
use std::env;
use std::sync::Arc;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Ed25519 public verification key baked into the binary.
///
/// The private counterpart is held in the Deckwatch license-issuance KMS and
/// never leaves it. Rotating this key requires a new Deckwatch release —
/// intentional, per `LICENSING_STRATEGY.md` §2.2.
///
/// The value below is the DEVELOPMENT key used for local testing. Production
/// builds MUST override this at compile time via the `DECKWATCH_LICENSE_PUBKEY`
/// build script (see `build.rs`) or by patching this constant in a release
/// commit. Never ship the dev key to customers.
pub const EMBEDDED_PUBLIC_KEY: [u8; 32] = [
    0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef,
    0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef,
];

/// How long after `expires_at` Pro features stay live before going dark.
/// Matches `LICENSING_STRATEGY.md` §2.3. Community is unaffected.
pub const GRACE_PERIOD_SECS: i64 = 30 * 24 * 60 * 60;

/// Environment variable that holds a raw signed license blob (base64url of
/// `<json>.<signature>`). The K8s Secret loader writes the same format to
/// `/etc/deckwatch/license` and exports it into this env var via the pod
/// spec, so both code paths converge on one input format.
pub const LICENSE_ENV_VAR: &str = "DECKWATCH_LICENSE_KEY";

/// Fallback path a K8s Secret mount is expected at when the env var is unset.
pub const LICENSE_SECRET_PATH: &str = "/etc/deckwatch/license/license.jwt";

/// Pricing tier. `Community` is the always-free default and is what the
/// runtime falls back to on any failure path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Community,
    Pro,
    Enterprise,
}

impl Tier {
    /// Feature names the tier grants natively (before considering the
    /// explicit `features` allowlist on the license). Used by
    /// [`Entitlements::from_license`] so a bare "tier: pro" license grants
    /// the full Pro feature set even if the `features` array is empty.
    pub fn default_features(self) -> &'static [&'static str] {
        match self {
            Tier::Community => &[],
            Tier::Pro => &[
                "ai_diagnostics",
                "ai_code_fix",
                "sso_multi_tenant",
                "webhook_notifications",
                "prometheus_charts",
                "extended_log_search",
                "audit_30d",
            ],
            Tier::Enterprise => &[
                "ai_diagnostics",
                "ai_code_fix",
                "sso_multi_tenant",
                "webhook_notifications",
                "prometheus_charts",
                "extended_log_search",
                "audit_30d",
                "multi_cluster",
                "audit_immutable",
                "custom_branding",
                "advanced_rbac",
                "compliance_packs",
                "air_gapped_installer",
            ],
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Community => "community",
            Tier::Pro => "pro",
            Tier::Enterprise => "enterprise",
        }
    }
}

/// Numeric caps a license carries. Enforced softly per `LICENSING_STRATEGY.md`
/// §2.5 — exceeding a limit warns, it does not block cluster control.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub max_users: Option<u32>,
    #[serde(default)]
    pub max_clusters: Option<u32>,
}

/// The signed payload embedded in a license blob. Kept intentionally minimal
/// so it fits in an environment variable or K8s Secret without paging.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicensePayload {
    /// Issuer — always `https://license.deckwatch.io` in production.
    #[serde(default)]
    pub iss: String,
    /// Customer UUID.
    #[serde(default)]
    pub sub: String,
    /// Issued-at (unix seconds).
    pub iat: i64,
    /// Expiry (unix seconds). Pro features grant a 30-day grace after this
    /// per `LICENSING_STRATEGY.md` §2.3.
    pub exp: i64,
    /// License ID — used for revocation lookups against the phone-home service.
    #[serde(default)]
    pub jti: String,
    pub tier: Tier,
    /// Explicit feature allowlist. Additive to `tier.default_features()`.
    /// Lets Deckwatch grant a customer a single Enterprise feature (e.g.
    /// custom_branding) on a Pro contract without a whole tier upgrade.
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub limits: Limits,
    /// Cosmetic — shown in the Settings UI so admins recognize their license.
    /// Do not put PII beyond an org name and contact email here; the blob
    /// ends up in customer git repos.
    #[serde(default)]
    pub customer: Option<CustomerInfo>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CustomerInfo {
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub contact: String,
}

/// Signed license: payload + Ed25519 signature over the canonical JSON bytes.
#[derive(Clone, Debug)]
pub struct License {
    pub payload: LicensePayload,
    /// Raw signature bytes as extracted from the wire format.
    pub signature: [u8; 64],
    /// Canonical JSON bytes that the signature covers. Kept so a caller can
    /// re-verify without reserializing (serialization would introduce
    /// key-ordering ambiguity that would break signature verification).
    pub payload_json: Vec<u8>,
}

impl License {
    /// Verify the license signature against [`EMBEDDED_PUBLIC_KEY`].
    pub fn verify(&self) -> Result<(), LicenseError> {
        self.verify_with_key(&EMBEDDED_PUBLIC_KEY)
    }

    /// The check behind [`License::verify`], taking the public key as a
    /// parameter so tests can run the exact production path with a key whose
    /// private half they hold. Deliberately private: production code must
    /// only ever verify against [`EMBEDDED_PUBLIC_KEY`].
    fn verify_with_key(&self, public_key: &[u8; 32]) -> Result<(), LicenseError> {
        let key = VerifyingKey::from_bytes(public_key)
            .map_err(|e| LicenseError::InvalidPublicKey(e.to_string()))?;
        let sig = Signature::from_bytes(&self.signature);
        key.verify(&self.payload_json, &sig)
            .map_err(|_| LicenseError::BadSignature)
    }
}

/// Errors raised during license load/verify. Callers MUST translate any of
/// these into `Entitlements::community()` rather than propagating — the
/// runtime must never hard-fail on a license problem. See module docs.
#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error(
        "no license source found (env {LICENSE_ENV_VAR} unset and {LICENSE_SECRET_PATH} missing)"
    )]
    NotFound,
    #[error("license blob malformed: {0}")]
    Malformed(String),
    #[error("license signature does not verify against the embedded public key")]
    BadSignature,
    #[error("embedded public key is invalid: {0}")]
    InvalidPublicKey(String),
}

/// Load a signed license from the environment or a K8s Secret mount, verify
/// its signature, and return the parsed [`License`]. Returns `Ok(None)` if no
/// source is configured — the caller should treat no-license as Community.
/// Returns `Err` only for concrete failures the caller may want to log; the
/// caller MUST still degrade to Community on error.
pub fn load_license() -> Result<Option<License>, LicenseError> {
    let raw = match env::var(LICENSE_ENV_VAR) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => match std::fs::read_to_string(LICENSE_SECRET_PATH) {
            Ok(s) if !s.trim().is_empty() => s,
            _ => return Ok(None),
        },
    };

    let license = parse_license_blob(raw.trim())?;
    license.verify()?;
    Ok(Some(license))
}

/// Wire format is `<base64url(payload_json)>.<base64url(signature)>`.
/// Same delimiter shape as JWS Compact Serialization but without a header,
/// because the algorithm is fixed at Ed25519 and versioning happens at the
/// public-key level.
fn parse_license_blob(blob: &str) -> Result<License, LicenseError> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;

    let (payload_b64, sig_b64) = blob
        .split_once('.')
        .ok_or_else(|| LicenseError::Malformed("missing '.' separator".into()))?;

    let payload_json = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|e| LicenseError::Malformed(format!("payload base64: {e}")))?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64.as_bytes())
        .map_err(|e| LicenseError::Malformed(format!("signature base64: {e}")))?;

    if sig_bytes.len() != 64 {
        return Err(LicenseError::Malformed(format!(
            "signature is {} bytes, expected 64",
            sig_bytes.len()
        )));
    }
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&sig_bytes);

    let payload: LicensePayload = serde_json::from_slice(&payload_json)
        .map_err(|e| LicenseError::Malformed(format!("payload json: {e}")))?;

    Ok(License {
        payload,
        signature,
        payload_json,
    })
}

/// Runtime feature-flag object cloned into `AppState` at startup and consulted
/// by the [`require_entitlement`] middleware and any handler that gates
/// behavior on tier.
#[derive(Clone, Debug)]
pub struct Entitlements {
    inner: Arc<EntitlementsInner>,
}

#[derive(Debug)]
struct EntitlementsInner {
    tier: Tier,
    features: HashSet<String>,
    limits: Limits,
    /// `None` for the built-in Community fallback (never expires).
    expires_at: Option<Timestamp>,
    /// True when the underlying license has passed its `exp` but is still
    /// within the 30-day grace window. Surfaced in the license API so the
    /// UI can render a "your license expired N days ago, renew soon" banner
    /// without changing runtime behavior.
    in_grace: bool,
    customer: Option<CustomerInfo>,
    /// The license ID (`jti`) of the loaded license, if any. Kept for support tickets.
    license_id: Option<String>,
}

impl Entitlements {
    /// Free-forever default. Returned on any failure path.
    pub fn community() -> Self {
        Self {
            inner: Arc::new(EntitlementsInner {
                tier: Tier::Community,
                features: HashSet::new(),
                limits: Limits::default(),
                expires_at: None,
                in_grace: false,
                customer: None,
                license_id: None,
            }),
        }
    }

    /// Build entitlements from a verified license, evaluated at `now`.
    /// - Before `exp`: full feature set active.
    /// - `exp` <= now < `exp + GRACE_PERIOD_SECS`: full feature set stays
    ///   active, `in_grace=true` so the UI can nag.
    /// - `now >= exp + GRACE_PERIOD_SECS`: silently downgrade to Community.
    pub fn from_license(license: &License, now: Timestamp) -> Self {
        let exp = Timestamp::from_second(license.payload.exp).unwrap_or(now);
        let grace_cutoff =
            Timestamp::from_second(license.payload.exp + GRACE_PERIOD_SECS).unwrap_or(now);

        if now >= grace_cutoff {
            // Hard-expired past grace: fall back to Community. Do NOT hard-fail;
            // cluster control must keep working. Preserve the customer info so
            // the UI can still render "expired on 2026-01-01, please renew".
            return Self {
                inner: Arc::new(EntitlementsInner {
                    tier: Tier::Community,
                    features: HashSet::new(),
                    limits: Limits::default(),
                    expires_at: Some(exp),
                    in_grace: false,
                    customer: license.payload.customer.clone(),
                    license_id: Some(license.payload.jti.clone()).filter(|s| !s.is_empty()),
                }),
            };
        }

        let in_grace = now >= exp;
        let tier = license.payload.tier;

        let mut features: HashSet<String> = tier
            .default_features()
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        for extra in &license.payload.features {
            features.insert(extra.clone());
        }

        Self {
            inner: Arc::new(EntitlementsInner {
                tier,
                features,
                limits: license.payload.limits.clone(),
                expires_at: Some(exp),
                in_grace,
                customer: license.payload.customer.clone(),
                license_id: Some(license.payload.jti.clone()).filter(|s| !s.is_empty()),
            }),
        }
    }

    pub fn tier(&self) -> Tier {
        self.inner.tier
    }

    pub fn has(&self, feature: &str) -> bool {
        self.inner.features.contains(feature)
    }

    pub fn features(&self) -> Vec<String> {
        let mut v: Vec<String> = self.inner.features.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    pub fn expires_at(&self) -> Option<Timestamp> {
        self.inner.expires_at
    }

    pub fn in_grace(&self) -> bool {
        self.inner.in_grace
    }

    pub fn customer(&self) -> Option<&CustomerInfo> {
        self.inner.customer.as_ref()
    }

    pub fn license_id(&self) -> Option<&str> {
        self.inner.license_id.as_deref()
    }

    /// The minimum tier a feature belongs to. Used by [`require_entitlement`]
    /// so the 403 response can advertise the tier the customer must upgrade
    /// to. Additive customer-specific grants (feature in a lower tier's
    /// license `features` array) do not change what tier we _advertise_.
    pub fn required_tier(feature: &str) -> Tier {
        if Tier::Enterprise.default_features().contains(&feature)
            && !Tier::Pro.default_features().contains(&feature)
        {
            Tier::Enterprise
        } else if Tier::Pro.default_features().contains(&feature) {
            Tier::Pro
        } else {
            // Unknown or Community feature — shouldn't happen since only
            // Pro/Enterprise features are ever gated. Default to Pro so the
            // response is at least non-misleading.
            Tier::Pro
        }
    }
}

/// Load-once entry point called from `main.rs`. On any failure it logs at
/// WARN and returns Community entitlements so the server keeps booting.
pub fn init_entitlements() -> Entitlements {
    match load_license() {
        Ok(None) => {
            tracing::info!("no deckwatch license configured; running with Community entitlements");
            Entitlements::community()
        }
        Ok(Some(license)) => {
            let now = Timestamp::now();
            let ent = Entitlements::from_license(&license, now);
            tracing::info!(
                tier = ent.tier().as_str(),
                expires_at = ?ent.expires_at(),
                in_grace = ent.in_grace(),
                license_id = ent.license_id(),
                "loaded deckwatch license"
            );
            ent
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to load deckwatch license; degrading to Community. \
                 Cluster control is unaffected."
            );
            Entitlements::community()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn community_has_no_features() {
        let ent = Entitlements::community();
        assert_eq!(ent.tier(), Tier::Community);
        assert!(!ent.has("ai_diagnostics"));
        assert!(!ent.has("multi_cluster"));
        assert!(!ent.in_grace());
    }

    #[test]
    fn pro_tier_grants_pro_features_not_enterprise() {
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-1".into(),
            iat: 1_700_000_000,
            exp: 9_999_999_999,
            jti: "lic-1".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits::default(),
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let ent =
            Entitlements::from_license(&license, Timestamp::from_second(1_800_000_000).unwrap());
        assert_eq!(ent.tier(), Tier::Pro);
        assert!(ent.has("ai_diagnostics"));
        assert!(ent.has("webhook_notifications"));
        assert!(!ent.has("multi_cluster"));
        assert!(!ent.in_grace());
    }

    #[test]
    fn enterprise_includes_pro_features() {
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-2".into(),
            iat: 1_700_000_000,
            exp: 9_999_999_999,
            jti: "lic-2".into(),
            tier: Tier::Enterprise,
            features: vec![],
            limits: Limits::default(),
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let ent =
            Entitlements::from_license(&license, Timestamp::from_second(1_800_000_000).unwrap());
        assert!(ent.has("ai_diagnostics")); // Pro feature
        assert!(ent.has("multi_cluster")); // Enterprise feature
        assert!(ent.has("custom_branding")); // Enterprise feature
    }

    #[test]
    fn explicit_features_are_additive() {
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-3".into(),
            iat: 1_700_000_000,
            exp: 9_999_999_999,
            jti: "lic-3".into(),
            tier: Tier::Pro,
            features: vec!["custom_branding".into()],
            limits: Limits::default(),
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let ent =
            Entitlements::from_license(&license, Timestamp::from_second(1_800_000_000).unwrap());
        assert!(ent.has("custom_branding"));
        assert!(ent.has("ai_diagnostics"));
    }

    #[test]
    fn grace_period_keeps_features_active() {
        let exp = 1_800_000_000i64;
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-4".into(),
            iat: 1_700_000_000,
            exp,
            jti: "lic-4".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits::default(),
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let one_day_after = Timestamp::from_second(exp + 24 * 60 * 60).unwrap();
        let ent = Entitlements::from_license(&license, one_day_after);
        assert_eq!(ent.tier(), Tier::Pro);
        assert!(ent.has("ai_diagnostics"));
        assert!(ent.in_grace());
    }

    #[test]
    fn past_grace_period_falls_back_to_community() {
        let exp = 1_800_000_000i64;
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-5".into(),
            iat: 1_700_000_000,
            exp,
            jti: "lic-5".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits::default(),
            customer: Some(CustomerInfo {
                org: "Acme".into(),
                contact: "ops@acme.example".into(),
            }),
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let past_grace = Timestamp::from_second(exp + 60 * 24 * 60 * 60).unwrap();
        let ent = Entitlements::from_license(&license, past_grace);
        assert_eq!(ent.tier(), Tier::Community);
        assert!(!ent.has("ai_diagnostics"));
        assert!(ent.customer().is_some());
    }

    #[test]
    fn required_tier_maps_features_to_advertisement() {
        assert_eq!(Entitlements::required_tier("ai_diagnostics"), Tier::Pro);
        assert_eq!(
            Entitlements::required_tier("multi_cluster"),
            Tier::Enterprise
        );
        assert_eq!(
            Entitlements::required_tier("custom_branding"),
            Tier::Enterprise
        );
        assert_eq!(
            Entitlements::required_tier("webhook_notifications"),
            Tier::Pro
        );
    }

    #[test]
    fn round_trip_signature_verifies_with_signing_key() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;

        // rand 0.10's ThreadRng implements rand_core 0.10's `CryptoRng`, which
        // is what ed25519-dalek 3's `generate` requires.
        let key = SigningKey::generate(&mut rand::rng());
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-6".into(),
            iat: 1_700_000_000,
            exp: 9_999_999_999,
            jti: "lic-6".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits::default(),
            customer: None,
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let sig = key.sign(&json);
        let blob = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&json),
            URL_SAFE_NO_PAD.encode(sig.to_bytes()),
        );

        let parsed = parse_license_blob(&blob).expect("parses");
        assert_eq!(parsed.payload.tier, Tier::Pro);
        // Verify against the signing key we actually used (not EMBEDDED_PUBLIC_KEY).
        let vk = key.verifying_key();
        vk.verify(
            &parsed.payload_json,
            &Signature::from_bytes(&parsed.signature),
        )
        .expect("signature verifies against original signing key");
        // Same check through the production verification path.
        parsed
            .verify_with_key(vk.as_bytes())
            .expect("production verify path accepts the signing key");
    }

    #[test]
    fn malformed_blob_returns_error() {
        assert!(matches!(
            parse_license_blob("garbage"),
            Err(LicenseError::Malformed(_))
        ));
        assert!(matches!(
            parse_license_blob("aaa.bbb"),
            Err(LicenseError::Malformed(_))
        ));
    }

    #[test]
    fn grace_boundary_exact_is_community() {
        // At exactly exp + grace, we're past grace — half-open interval.
        let exp = 1_000_000_000i64;
        let payload = LicensePayload {
            iss: "test".into(),
            sub: "cust-7".into(),
            iat: 900_000_000,
            exp,
            jti: "lic-7".into(),
            tier: Tier::Pro,
            features: vec![],
            limits: Limits::default(),
            customer: None,
        };
        let license = License {
            payload: payload.clone(),
            signature: [0u8; 64],
            payload_json: serde_json::to_vec(&payload).unwrap(),
        };
        let at_boundary = Timestamp::from_second(exp + GRACE_PERIOD_SECS).unwrap();
        let ent = Entitlements::from_license(&license, at_boundary);
        assert_eq!(ent.tier(), Tier::Community);
    }

    // Known-answer vector produced with ed25519-dalek 2.2.0 (main before the
    // 3.x upgrade) and cross-checked with OpenSSL via Python `cryptography`.
    // Ed25519 signing is deterministic (RFC 8032), so any compliant
    // implementation must reproduce these bytes exactly.
    const KAT_SEED: [u8; 32] = [42u8; 32];
    const KAT_PUBLIC_KEY_HEX: &str =
        "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61";
    const KAT_PAYLOAD: &[u8] = br#"{"iss":"https://license.deckwatch.io","sub":"cust-kat","iat":1700000000,"exp":9999999999,"jti":"lic-kat","tier":"enterprise","features":["custom_branding"],"limits":{"max_users":50,"max_clusters":3},"customer":{"org":"Acme","contact":"ops@acme.example"}}"#;
    const KAT_SIGNATURE_HEX: &str = "ffc3336116bb29d7dbb1f6720436bf5e779677f1f22398b911a42e4ba7111f5820f37ac172ecdfc7016ecbac054b19bed82a5b6d9cb7e3bf5f1ac7a45a502b0f";
    const KAT_BLOB: &str = "eyJpc3MiOiJodHRwczovL2xpY2Vuc2UuZGVja3dhdGNoLmlvIiwic3ViIjoiY3VzdC1rYXQiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6OTk5OTk5OTk5OSwianRpIjoibGljLWthdCIsInRpZXIiOiJlbnRlcnByaXNlIiwiZmVhdHVyZXMiOlsiY3VzdG9tX2JyYW5kaW5nIl0sImxpbWl0cyI6eyJtYXhfdXNlcnMiOjUwLCJtYXhfY2x1c3RlcnMiOjN9LCJjdXN0b21lciI6eyJvcmciOiJBY21lIiwiY29udGFjdCI6Im9wc0BhY21lLmV4YW1wbGUifX0._8MzYRa7KdfbsfZyBDa_XneWd_HyI5i5EaQuS6cRH1gg83rBcuzfxwFuy6wFSxm-2CpbbZy3479fGsekWlArDw";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn kat_public_key() -> [u8; 32] {
        SigningKey::from_bytes(&KAT_SEED).verifying_key().to_bytes()
    }

    #[test]
    fn signing_matches_previous_release_vector() {
        let key = SigningKey::from_bytes(&KAT_SEED);
        assert_eq!(hex(key.verifying_key().as_bytes()), KAT_PUBLIC_KEY_HEX);
        assert_eq!(hex(&key.sign(KAT_PAYLOAD).to_bytes()), KAT_SIGNATURE_HEX);
    }

    #[test]
    fn previous_release_blob_verifies_via_production_path() {
        let license = parse_license_blob(KAT_BLOB).expect("parses");
        assert_eq!(license.payload_json, KAT_PAYLOAD);
        assert_eq!(hex(&license.signature), KAT_SIGNATURE_HEX);
        assert_eq!(license.payload.tier, Tier::Enterprise);
        license
            .verify_with_key(&kat_public_key())
            .expect("valid signature verifies");
    }

    #[test]
    fn signature_checks_reject_tampering() {
        let vk = kat_public_key();
        let good = parse_license_blob(KAT_BLOB).unwrap();
        let rejected = |l: &License, key: &[u8; 32]| {
            matches!(l.verify_with_key(key), Err(LicenseError::BadSignature))
        };

        // Payload edited after signing ("enterprise" -> "enterprisf").
        let mut payload = good.clone();
        let pos = payload
            .payload_json
            .windows(10)
            .position(|w| w == b"enterprise")
            .unwrap();
        payload.payload_json[pos + 9] ^= 0x03;
        assert!(rejected(&payload, &vk), "tampered payload");

        // R component of the signature altered.
        let mut r = good.clone();
        r.signature[0] ^= 0x01;
        assert!(rejected(&r, &vk), "tampered R");

        // Non-canonical s with the high bit set.
        let mut high = good.clone();
        high.signature[63] ^= 0x80;
        assert!(rejected(&high, &vk), "high-bit s");

        // s + L: same scalar mod L, non-canonical encoding. Strict (non
        // `legacy_compatibility`) verification must reject this malleated
        // signature, as ed25519-dalek 2.2.0 did.
        const L: [u8; 32] = [
            0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9,
            0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
        ];
        let mut malleated = good.clone();
        let mut carry = 0u16;
        for (b, l) in malleated.signature[32..].iter_mut().zip(L) {
            let sum = u16::from(*b) + u16::from(l) + carry;
            *b = sum as u8;
            carry = sum >> 8;
        }
        assert_eq!(carry, 0);
        assert!(rejected(&malleated, &vk), "s + L");

        // Valid signature, but from a different key.
        let other = SigningKey::from_bytes(&[7u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(rejected(&good, &other), "foreign key");

        // The untouched blob still verifies.
        assert!(good.verify_with_key(&vk).is_ok());
    }

    #[test]
    fn verify_checks_against_embedded_public_key() {
        // The in-tree EMBEDDED_PUBLIC_KEY is the 0xdeadbeef development
        // placeholder, which is not a valid Edwards point: ed25519-dalek 2.2.0
        // refused to decompress it, so `verify()` returned InvalidPublicKey for
        // every license and the runtime degraded to Community. 3.x must keep
        // rejecting it the same way.
        let placeholder: [u8; 32] = std::array::from_fn(|i| [0xde, 0xad, 0xbe, 0xef][i % 4]);
        let license = parse_license_blob(KAT_BLOB).unwrap();
        assert!(matches!(
            license.verify_with_key(&placeholder),
            Err(LicenseError::InvalidPublicKey(_))
        ));

        // `verify()` must use the embedded key, never the KAT key.
        let err = license
            .verify()
            .expect_err("KAT key is not the embedded key");
        if EMBEDDED_PUBLIC_KEY == placeholder {
            assert!(matches!(err, LicenseError::InvalidPublicKey(_)), "{err}");
        } else {
            assert!(matches!(err, LicenseError::BadSignature), "{err}");
        }
    }
}
