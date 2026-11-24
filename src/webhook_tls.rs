#![allow(dead_code, unused_imports)]
//! Self-signed TLS bootstrap for the admission webhook listener.
//!
//! Every startup mints a fresh CA + server cert with `rcgen`. The pair
//! is written to disk (so axum-server can read the PEM files) and the
//! CA DER is returned to the caller for injection into the
//! `ValidatingWebhookConfiguration.webhooks[].clientConfig.caBundle`.
//!
//! Rotation strategy in v1: certs are short-lived (~24h). Because the
//! webhook config's `caBundle` is refreshed by the same process every
//! startup, a rolling restart is the only supported rotation event.
//! Longer-lived certs + hot-swap are called out in the design doc §5.2
//! as follow-up work.
//!
//! For production, operators should install cert-manager and let it
//! own the cert lifecycle (Helm-managed path — §5.1). This module is
//! the "batteries included" path so `webhook.enabled: true` works out
//! of the box in dev / self-managed clusters.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rcgen::{
    Certificate, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};

/// Layout of the generated material on disk. All three paths live in
/// the same directory so tests can point at a `TempDir` and clean up
/// with one call.
pub struct WebhookTls {
    pub ca_pem_path: PathBuf,
    pub server_cert_path: PathBuf,
    pub server_key_path: PathBuf,
    pub ca_bundle_b64: String,
    pub ca_pem: String,
    pub not_after_unix: i64,
}

/// Generate a fresh self-signed CA + server cert covering the webhook's
/// in-cluster DNS names. `service_name`/`namespace` are the Service
/// deckwatch mounts in front of the webhook (see the Helm template).
/// `out_dir` is created if missing; existing files are overwritten.
///
/// Returns paths + the base64-encoded CA bundle ready to drop into a
/// `ValidatingWebhookConfiguration`.
pub fn generate(service_name: &str, namespace: &str, out_dir: &Path) -> Result<WebhookTls> {
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating webhook cert dir {}", out_dir.display()))?;

    // ---- CA ---------------------------------------------------------
    let mut ca_params = CertificateParams::new(Vec::new()).context("building CA params")?;
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "deckwatch-webhook-ca");
    ca_dn.push(DnType::OrganizationName, "deckwatch");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    // Short-lived (~24h). See module docstring for the rotation story.
    ca_params.not_before = time::OffsetDateTime::now_utc();
    ca_params.not_after = ca_params.not_before + time::Duration::hours(24);

    let cert_not_before = ca_params.not_before;
    let cert_not_after = ca_params.not_after;

    let ca_key = KeyPair::generate().context("generating CA key")?;
    let ca_cert: Certificate = ca_params.self_signed(&ca_key).context("self-signing CA")?;
    // rcgen 0.14 signs with an `Issuer` (subject DN, key-identifier method,
    // key usages + signing key) instead of a `Certificate`. Building it from
    // the same params/key as the self-signed CA above keeps the server
    // cert's issuer name byte-identical to the CA subject.
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    // ---- Server cert ------------------------------------------------
    let dns_names = webhook_dns_names(service_name, namespace);
    let mut srv_params =
        CertificateParams::new(dns_names.clone()).context("building server cert params")?;
    let mut srv_dn = DistinguishedName::new();
    srv_dn.push(
        DnType::CommonName,
        format!("{service_name}.{namespace}.svc"),
    );
    srv_dn.push(DnType::OrganizationName, "deckwatch");
    srv_params.distinguished_name = srv_dn;
    srv_params.not_before = cert_not_before;
    srv_params.not_after = cert_not_after;
    srv_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    srv_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];

    let srv_key = KeyPair::generate().context("generating server key")?;
    let srv_cert = srv_params
        .signed_by(&srv_key, &ca_issuer)
        .context("signing server cert with CA")?;

    // ---- Write to disk ---------------------------------------------
    let ca_pem = ca_cert.pem();
    let server_cert_pem = srv_cert.pem();
    let server_key_pem = srv_key.serialize_pem();

    let ca_pem_path = out_dir.join("ca.crt");
    let server_cert_path = out_dir.join("tls.crt");
    let server_key_path = out_dir.join("tls.key");

    std::fs::write(&ca_pem_path, &ca_pem).context("writing ca.crt")?;
    std::fs::write(&server_cert_path, &server_cert_pem).context("writing tls.crt")?;
    std::fs::write(&server_key_path, &server_key_pem).context("writing tls.key")?;

    let ca_bundle_b64 = B64.encode(ca_pem.as_bytes());

    let not_after_unix = cert_not_after.unix_timestamp();
    let _ = SystemTime::now().duration_since(UNIX_EPOCH); // sanity import guard

    Ok(WebhookTls {
        ca_pem_path,
        server_cert_path,
        server_key_path,
        ca_bundle_b64,
        ca_pem,
        not_after_unix,
    })
}

/// The in-cluster DNS names the API server may use when calling us.
/// The API server is strict: the server cert must have every SAN it
/// might try to dial, so we include short + `.svc` + `.svc.cluster.local`.
fn webhook_dns_names(service: &str, namespace: &str) -> Vec<String> {
    vec![
        service.to_string(),
        format!("{service}.{namespace}"),
        format!("{service}.{namespace}.svc"),
        format!("{service}.{namespace}.svc.cluster.local"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generate_writes_pem_files() {
        let dir = TempDir::new().unwrap();
        let out = generate("deckwatch-webhook", "deckwatch", dir.path()).unwrap();
        assert!(out.ca_pem_path.exists());
        assert!(out.server_cert_path.exists());
        assert!(out.server_key_path.exists());
        let ca = std::fs::read_to_string(&out.ca_pem_path).unwrap();
        assert!(ca.contains("BEGIN CERTIFICATE"));
        assert!(!out.ca_bundle_b64.is_empty());
    }

    #[test]
    fn dns_names_cover_svc_variants() {
        let names = webhook_dns_names("deckwatch-webhook", "deckwatch");
        assert!(names.iter().any(|n| n == "deckwatch-webhook.deckwatch.svc"));
        assert!(names
            .iter()
            .any(|n| n == "deckwatch-webhook.deckwatch.svc.cluster.local"));
    }

    /// Decode a single-certificate PEM document to DER.
    fn cert_pem_to_der(pem: &[u8]) -> Vec<u8> {
        let (rest, pem) = x509_parser::pem::parse_x509_pem(pem).expect("valid PEM");
        assert!(rest.iter().all(u8::is_ascii_whitespace), "one PEM block");
        assert_eq!(pem.label, "CERTIFICATE");
        pem.contents
    }

    /// The generated chain must be what the kube-apiserver expects: a
    /// self-signed CA (the caBundle) that issued a serverAuth cert for
    /// every in-cluster Service name, both valid for the same ~24h window.
    #[test]
    fn server_cert_verifies_against_ca_bundle() {
        use rustls::client::danger::ServerCertVerifier;
        use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
        use x509_parser::prelude::*;

        let (service, namespace) = ("deckwatch-webhook", "deckwatch");
        let dir = TempDir::new().unwrap();
        let out = generate(service, namespace, dir.path()).unwrap();

        // caBundle (what the API server trusts) is the PEM written to ca.crt.
        let bundle_pem = B64.decode(&out.ca_bundle_b64).unwrap();
        assert_eq!(bundle_pem, std::fs::read(&out.ca_pem_path).unwrap());
        assert_eq!(bundle_pem, out.ca_pem.as_bytes());
        let ca_der = cert_pem_to_der(&bundle_pem);
        let srv_der = cert_pem_to_der(&std::fs::read(&out.server_cert_path).unwrap());
        let (_, ca) = parse_x509_certificate(&ca_der).unwrap();
        let (_, srv) = parse_x509_certificate(&srv_der).unwrap();

        // CA: self-issued + self-signed, critical CA:TRUE without a path
        // length limit, allowed to sign certs and CRLs.
        assert_eq!(ca.subject(), ca.issuer());
        ca.verify_signature(None).expect("CA is self-signed");
        let bc = ca
            .basic_constraints()
            .unwrap()
            .expect("CA basicConstraints");
        assert!(bc.critical);
        assert!(bc.value.ca);
        assert_eq!(bc.value.path_len_constraint, None);
        let ku = ca.key_usage().unwrap().expect("CA keyUsage").value;
        assert!(ku.key_cert_sign() && ku.crl_sign() && ku.digital_signature());

        // Server cert: issued and signed by that CA, not a CA itself.
        assert_eq!(srv.issuer(), ca.subject());
        srv.verify_signature(Some(ca.public_key()))
            .expect("server cert is signed by the CA key");
        assert!(!srv.is_ca());
        let cn: Vec<_> = srv
            .subject()
            .iter_common_name()
            .map(|cn| cn.as_str().unwrap().to_string())
            .collect();
        assert_eq!(cn, [format!("{service}.{namespace}.svc")]);
        let ku = srv.key_usage().unwrap().expect("server keyUsage").value;
        assert!(ku.digital_signature() && ku.key_encipherment());
        assert!(!ku.key_cert_sign());
        let eku = srv.extended_key_usage().unwrap().expect("server EKU").value;
        assert!(eku.server_auth);
        let sans: Vec<String> = srv
            .subject_alternative_name()
            .unwrap()
            .expect("server SAN")
            .value
            .general_names
            .iter()
            .map(|gn| match gn {
                GeneralName::DNSName(name) => name.to_string(),
                other => panic!("unexpected SAN {other:?}"),
            })
            .collect();
        assert_eq!(sans, webhook_dns_names(service, namespace));

        // Same 24h validity window on both certs; not_after is reported.
        for cert in [&ca, &srv] {
            let validity = cert.validity();
            assert_eq!(
                validity.not_after.timestamp() - validity.not_before.timestamp(),
                24 * 60 * 60
            );
            assert_eq!(validity.not_after.timestamp(), out.not_after_unix);
        }

        // tls.key is the private half of tls.crt.
        let srv_key = KeyPair::from_pem(&std::fs::read_to_string(&out.server_key_path).unwrap())
            .expect("tls.key parses");
        assert_eq!(
            srv_key.public_key_raw(),
            srv.public_key().subject_public_key.data.as_ref()
        );

        // A TLS client that trusts only the caBundle accepts the serving
        // cert for every in-cluster name and rejects any other name.
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(ca_der)).unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            std::sync::Arc::new(roots),
            std::sync::Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();
        let leaf = CertificateDer::from(srv_der);
        for name in webhook_dns_names(service, namespace) {
            let server_name = ServerName::try_from(name.clone()).unwrap();
            verifier
                .verify_server_cert(&leaf, &[], &server_name, &[], UnixTime::now())
                .unwrap_or_else(|e| panic!("server cert rejected for {name}: {e}"));
        }
        let other = ServerName::try_from("deckwatch-webhook.other.svc").unwrap();
        assert!(verifier
            .verify_server_cert(&leaf, &[], &other, &[], UnixTime::now())
            .is_err());
    }
}
