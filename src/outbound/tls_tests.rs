use super::*;

fn ca_pem(common_name: &str) -> String {
    let mut params = rcgen::CertificateParams::new(Vec::new()).expect("params");
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().expect("key");
    params.self_signed(&key).expect("self-sign").pem()
}

const PATH: &str = "/etc/deckwatch/extra-ca/ca.crt";

#[test]
fn loads_a_single_ca() {
    let bundle = ExtraCaBundle::from_pem(PATH, ca_pem("corp-root").as_bytes()).expect("valid");
    assert_eq!(bundle.len(), 1);
    assert!(!bundle.is_empty());
    assert_eq!(bundle.path(), Path::new(PATH));
}

#[test]
fn loads_every_certificate_of_a_bundle_and_skips_comments() {
    // OpenSSL-style bundles interleave comments with the PEM blocks.
    let pem = format!(
        "# Corp root\n{}\n# Corp issuing CA\n{}",
        ca_pem("corp-root"),
        ca_pem("corp-issuing")
    );
    let bundle = ExtraCaBundle::from_pem(PATH, pem.as_bytes()).expect("valid");
    assert_eq!(bundle.len(), 2);
    assert_eq!(bundle.certificates().len(), 2);
}

#[test]
fn rejects_an_empty_file_naming_it() {
    let err = ExtraCaBundle::from_pem(PATH, b"").expect_err("empty");
    assert!(matches!(err, ExtraCaError::Empty { .. }), "{err:?}");
    assert!(err.to_string().contains(PATH), "{err}");
}

#[test]
fn rejects_a_file_without_pem_blocks() {
    let err = ExtraCaBundle::from_pem(PATH, b"not a certificate\n").expect_err("garbage");
    assert!(matches!(err, ExtraCaError::Empty { .. }), "{err:?}");
}

#[test]
fn rejects_a_truncated_pem_block() {
    let pem = ca_pem("corp-root");
    let truncated = &pem[..pem.len() / 2];
    let err = ExtraCaBundle::from_pem(PATH, truncated.as_bytes()).expect_err("truncated");
    assert!(matches!(err, ExtraCaError::Pem { .. }), "{err:?}");
    assert!(err.to_string().contains(PATH), "{err}");
}

#[test]
fn rejects_a_certificate_block_that_is_not_x509() {
    let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    let err = ExtraCaBundle::from_pem(PATH, pem.as_bytes()).expect_err("not x509");
    assert!(
        matches!(err, ExtraCaError::Certificate { index: 1, .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains(PATH), "{err}");
}

#[test]
fn rejects_a_bundle_containing_a_private_key() {
    let key = rcgen::KeyPair::generate().expect("key");
    let pem = format!("{}{}", ca_pem("corp-root"), key.serialize_pem());
    let err = ExtraCaBundle::from_pem(PATH, pem.as_bytes()).expect_err("private key");
    assert!(
        matches!(err, ExtraCaError::PrivateKey { index: 2, .. }),
        "{err:?}"
    );
}

#[test]
fn load_reports_an_unreadable_file_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.pem");
    let err = ExtraCaBundle::load(&missing).expect_err("missing");
    assert!(matches!(err, ExtraCaError::Read { .. }), "{err:?}");
    assert!(
        err.to_string().contains(&missing.display().to_string()),
        "{err}"
    );
}

#[test]
fn load_reads_a_bundle_from_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.crt");
    std::fs::write(&path, ca_pem("corp-root")).expect("write");
    let bundle = ExtraCaBundle::load(&path).expect("valid");
    assert_eq!(bundle.path(), path.as_path());
    assert_eq!(bundle.len(), 1);
}

#[test]
fn certificates_convert_for_reqwest_and_object_store() {
    let pem = format!("{}{}", ca_pem("a"), ca_pem("b"));
    let bundle = ExtraCaBundle::from_pem(PATH, pem.as_bytes()).expect("valid");
    assert_eq!(bundle.reqwest_certificates().expect("reqwest").len(), 2);
    assert_eq!(
        bundle
            .object_store_certificates()
            .expect("object_store")
            .len(),
        2
    );
}
