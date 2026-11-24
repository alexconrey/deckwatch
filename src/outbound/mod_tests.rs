use super::*;

fn ca_bundle() -> ExtraCaBundle {
    let mut params = rcgen::CertificateParams::new(Vec::new()).expect("params");
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "corp-root");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().expect("key");
    let pem = params.self_signed(&key).expect("self-sign").pem();
    ExtraCaBundle::from_pem("/etc/deckwatch/extra-ca/ca.crt", pem.as_bytes()).expect("bundle")
}

#[test]
fn user_agent_names_deckwatch_and_its_version() {
    assert_eq!(
        user_agent(),
        format!("deckwatch/{}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn every_purpose_has_bounded_timeouts() {
    for purpose in Purpose::ALL {
        let t = purpose.timeouts();
        assert!(t.connect > Duration::ZERO, "{purpose:?}");
        assert!(t.connect < t.total, "{purpose:?}");
        assert!(!purpose.name().is_empty());
    }
}

#[test]
fn existing_timeouts_are_preserved() {
    // Totals the call sites set before the factory existed.
    assert_eq!(
        Purpose::PluginDownload.timeouts().total,
        Duration::from_secs(30)
    );
    assert_eq!(Purpose::Webhook.timeouts().total, Duration::from_secs(10));
    assert_eq!(
        Purpose::Observability.timeouts().total,
        Duration::from_secs(10)
    );
    // AI responses stream for minutes.
    assert!(Purpose::AiInference.timeouts().total >= Duration::from_secs(300));
}

#[test]
fn default_has_a_client_for_every_purpose_and_no_extra_ca() {
    let http = OutboundHttp::default();
    for purpose in Purpose::ALL {
        let _ = http.client(purpose);
    }
    assert!(http.extra_ca().is_none());
    assert!(!http.policy().allow_insecure);
}

#[test]
fn extra_ca_is_kept_and_applied() {
    let http = OutboundHttp::new(OutboundConfig {
        extra_ca: Some(ca_bundle()),
        policy: EndpointPolicy {
            allow_insecure: true,
        },
    })
    .expect("clients build with an extra CA");
    let bundle = http.extra_ca().expect("bundle");
    assert_eq!(bundle.len(), 1);
    assert_eq!(http.inner.object_store_roots.len(), 1);
    assert!(http.policy().allow_insecure);
    // object_store options build too (S3 registry, Bedrock credentials).
    let _ = http.object_store_client_options();
}

#[test]
fn clients_are_shared_not_rebuilt() {
    let http = OutboundHttp::default();
    let a = format!("{:?}", http.client(Purpose::Git));
    let b = format!("{:?}", http.clone().client(Purpose::Git));
    assert_eq!(a, b);
}

#[test]
fn debug_does_not_dump_clients() {
    let debug = format!("{:?}", OutboundHttp::default());
    assert!(debug.contains("OutboundHttp"), "{debug}");
    assert!(debug.contains("allow_insecure"), "{debug}");
}

#[test]
fn errors_name_the_setting() {
    let err = OutboundError::from(ExtraCaError::Empty {
        path: "/etc/ca.pem".into(),
    });
    let msg = err.to_string();
    assert!(msg.starts_with("DECKWATCH_EXTRA_CA_FILE: "), "{msg}");
    assert!(msg.contains("/etc/ca.pem"), "{msg}");
    let msg = OutboundError::InsecureFlag("maybe".into()).to_string();
    assert!(msg.contains("DECKWATCH_ALLOW_INSECURE_ENDPOINTS"), "{msg}");
}

#[test]
fn config_from_values_reads_the_bundle_and_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.crt");
    let mut params = rcgen::CertificateParams::new(Vec::new()).expect("params");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().expect("key");
    std::fs::write(&path, params.self_signed(&key).expect("cert").pem()).expect("write");

    let config =
        OutboundConfig::from_values(Some(path.clone().into_os_string()), Some("true".into()))
            .expect("valid");
    assert_eq!(config.extra_ca.expect("bundle").path(), path.as_path());
    assert!(config.policy.allow_insecure);
}

#[test]
fn config_treats_unset_or_empty_as_off() {
    for (file, flag) in [
        (None, None),
        (Some(std::ffi::OsString::new()), Some(String::new())),
        (None, Some("false".to_string())),
        (None, Some(" No ".to_string())),
        (None, Some("0".to_string())),
    ] {
        let config = OutboundConfig::from_values(file, flag).expect("valid");
        assert!(config.extra_ca.is_none());
        assert!(!config.policy.allow_insecure);
    }
}

#[test]
fn config_fails_loudly_on_a_bad_bundle_or_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.crt");
    let err = OutboundConfig::from_values(Some(missing.clone().into_os_string()), None)
        .expect_err("unreadable bundle");
    assert!(
        err.to_string().contains(&missing.display().to_string()),
        "{err}"
    );

    let garbage = dir.path().join("garbage.crt");
    std::fs::write(&garbage, "hello").expect("write");
    let err = OutboundConfig::from_values(Some(garbage.clone().into_os_string()), None)
        .expect_err("invalid bundle");
    assert!(
        err.to_string().contains(&garbage.display().to_string()),
        "{err}"
    );

    assert!(matches!(
        OutboundConfig::from_values(None, Some("sometimes".into())),
        Err(OutboundError::InsecureFlag(_))
    ));
}

#[derive(Debug)]
struct Layer {
    msg: &'static str,
    source: Option<Box<Layer>>,
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.msg)
    }
}

impl std::error::Error for Layer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

#[test]
fn error_chain_includes_every_source_once() {
    let err = Layer {
        msg: "error sending request for url (https://api.example.com/)",
        source: Some(Box::new(Layer {
            msg: "client error (Connect)",
            source: Some(Box::new(Layer {
                msg: "invalid peer certificate: UnknownIssuer",
                source: Some(Box::new(Layer {
                    // Already part of the message above: not repeated.
                    msg: "UnknownIssuer",
                    source: None,
                })),
            })),
        })),
    };
    assert_eq!(
        error_chain(&err),
        "error sending request for url (https://api.example.com/): client error (Connect): \
         invalid peer certificate: UnknownIssuer"
    );
}
