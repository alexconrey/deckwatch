//! Outbound HTTPS contract tests.
//!
//! Every upstream deckwatch talks to is replaced by a TLS server on
//! 127.0.0.1 whose certificate comes from a test CA. That CA is trusted only
//! through `DECKWATCH_EXTRA_CA_FILE`, the mechanism operators use for private
//! CAs, so the positive tests also prove the extra-CA path. Each test asserts
//! the request deckwatch sends (method, path, headers, body) and that it
//! parses the response.
//!
//! The negative tests cover what must fail: an untrusted CA, a hostname
//! mismatch, an expired certificate, a credential bound to another host
//! (refused before any connection is made) and a non-https endpoint.
//!
//! This is its own test binary because it sets process-wide environment
//! variables (`DECKWATCH_EXTRA_CA_FILE`, and on Linux `SSL_CERT_FILE`) once,
//! before any client is built. Run it alone with
//! `cargo test --test outbound_tls`.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::Digest;
use tokio::net::{TcpListener, TcpStream};

use deckwatch::anthropic::{exchange_sa_key_for_token, AnthropicClient, AwsAuth, ClientError};
use deckwatch::outbound::github::GitHubToken;
use deckwatch::outbound::{
    BindingError, CredentialSource, Endpoint, EndpointBinding, EndpointError, EndpointId,
    EndpointPolicy, EndpointSettings, OutboundConfig, OutboundHttp, Purpose, EXTRA_CA_FILE_ENV,
};
use deckwatch::testing;

// ---------------------------------------------------------------------------
// Test PKI
// ---------------------------------------------------------------------------

/// A CA that can issue server certificates.
struct TestCa {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

enum Validity {
    Current,
    Expired,
}

impl TestCa {
    fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::new()).expect("CA params");
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(30);
        let key = KeyPair::generate().expect("CA key");
        let pem = params.self_signed(&key).expect("self-signed CA").pem();
        Self {
            issuer: Issuer::new(params, key),
            pem,
        }
    }

    /// A serverAuth certificate for `names` (DNS names or IP literals).
    fn issue(&self, names: &[&str], validity: Validity) -> ServerCert {
        let sans: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let mut params = CertificateParams::new(sans).expect("leaf params");
        params.distinguished_name.push(DnType::CommonName, names[0]);
        // ECDSA keys: digitalSignature only (no keyEncipherment).
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = time::OffsetDateTime::now_utc();
        match validity {
            Validity::Current => {
                params.not_before = now - time::Duration::days(1);
                params.not_after = now + time::Duration::days(30);
            }
            Validity::Expired => {
                params.not_before = now - time::Duration::days(60);
                params.not_after = now - time::Duration::days(30);
            }
        }
        let key = KeyPair::generate().expect("leaf key");
        let cert = params.signed_by(&key, &self.issuer).expect("signed leaf");
        ServerCert {
            cert: cert.der().clone(),
            key: key.serialize_der(),
        }
    }
}

struct ServerCert {
    cert: CertificateDer<'static>,
    key: Vec<u8>,
}

/// Process-wide fixture. Built once, before any test builds a client, so the
/// environment variables it sets are never written while another thread
/// reads them.
struct Pki {
    _dir: tempfile::TempDir,
    /// Trusted through DECKWATCH_EXTRA_CA_FILE.
    extra: TestCa,
    extra_ca_file: PathBuf,
    /// Trusted through SSL_CERT_FILE, i.e. "the system store" (Linux).
    system: TestCa,
    /// Trusted by nobody.
    rogue: TestCa,
}

static PKI: LazyLock<Pki> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("tempdir");
    let extra = TestCa::new("deckwatch test extra CA");
    let system = TestCa::new("deckwatch test system CA");
    let rogue = TestCa::new("deckwatch test untrusted CA");

    let extra_ca_file = dir.path().join("extra-ca.pem");
    std::fs::write(&extra_ca_file, &extra.pem).expect("write extra CA");
    let system_ca_file = dir.path().join("system-ca.pem");
    std::fs::write(&system_ca_file, &system.pem).expect("write system CA");

    std::env::set_var(EXTRA_CA_FILE_ENV, &extra_ca_file);
    // OpenSSL (reqwest 0.12 / native-tls) and rustls-native-certs
    // (reqwest 0.13 / rustls-platform-verifier) both read SSL_CERT_FILE as
    // the system store on Linux. macOS uses the keychain instead, so the
    // system-roots test only runs on Linux.
    std::env::set_var("SSL_CERT_FILE", &system_ca_file);
    // Keep proxies configured on a developer machine out of the way.
    std::env::set_var("NO_PROXY", "*");

    let _ = rustls::crypto::ring::default_provider().install_default();

    Pki {
        _dir: dir,
        extra,
        extra_ca_file,
        system,
        rogue,
    }
});

fn pki() -> &'static Pki {
    &PKI
}

/// Clients exactly as deckwatch builds them at startup: from the
/// environment, so the test CA is trusted via DECKWATCH_EXTRA_CA_FILE.
fn trusted() -> OutboundHttp {
    let pki = pki();
    let http = OutboundHttp::from_env().expect("outbound clients from env");
    assert_eq!(
        http.extra_ca().expect("extra CA configured").path(),
        pki.extra_ca_file.as_path()
    );
    http
}

/// Clients with no extra CA: the test CA is unknown to them.
fn without_extra_ca() -> OutboundHttp {
    pki();
    OutboundHttp::new(OutboundConfig::default()).expect("outbound clients")
}

// ---------------------------------------------------------------------------
// TLS mock upstream
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Bytes,
}

impl Recorded {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .unwrap_or_else(|| panic!("missing {name} header: {:?}", self.headers))
            .to_str()
            .expect("ascii header")
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("JSON request body")
    }

    /// `application/x-www-form-urlencoded` body as ordered pairs.
    fn form(&self) -> Vec<(String, String)> {
        std::str::from_utf8(&self.body)
            .expect("UTF-8 form body")
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (form_decode(k), form_decode(v))
            })
            .collect()
    }
}

fn form_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("escape");
                out.push(u8::from_str_radix(hex, 16).expect("hex escape"));
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("UTF-8")
}

type Log = Arc<Mutex<Vec<Recorded>>>;

/// A TLS server on 127.0.0.1 that records every request it serves.
struct MockUpstream {
    addr: SocketAddr,
    log: Log,
    /// TCP connections accepted, whether or not the handshake completed.
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockUpstream {
    async fn start(cert: ServerCert, router: Router) -> Self {
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key)),
        )
        .expect("server certificate");
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().expect("addr");
        let log: Log = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let accepted = connections.clone();
        let accept_loop = async move {
            loop {
                let Ok((stream, peer)) = tcp.accept().await else {
                    continue;
                };
                accepted.fetch_add(1, Ordering::SeqCst);
                let acceptor = acceptor.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    // Failed handshakes (the client rejected our certificate)
                    // simply never reach the router.
                    if let Ok(tls) = acceptor.accept(stream).await {
                        let _ = tx.send((tls, peer)).await;
                    }
                });
            }
        };

        let app = router.layer(axum::middleware::from_fn_with_state(log.clone(), record));
        let listener = TlsListener { rx, addr };
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = accept_loop => {}
                _ = axum::serve(listener, app) => {}
            }
        });

        Self {
            addr,
            log,
            connections,
            task,
        }
    }

    /// `https://<host>:<port>` for this server.
    fn url(&self, host: &str) -> String {
        format!("https://{host}:{}", self.addr.port())
    }

    fn requests(&self) -> Vec<Recorded> {
        self.log.lock().expect("log").clone()
    }

    fn only_request(&self) -> Recorded {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "expected one request: {requests:#?}");
        requests.into_iter().next().expect("one request")
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

async fn record(State(log): State<Log>, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .expect("request body");
    log.lock().expect("log").push(Recorded {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().map(str::to_string),
        headers: parts.headers.clone(),
        body: bytes.clone(),
    });
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

struct TlsListener {
    rx: tokio::sync::mpsc::Receiver<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr)>,
    addr: SocketAddr,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(conn) => conn,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}

/// A certificate for the names the tests dial.
fn localhost_cert(ca: &TestCa) -> ServerCert {
    ca.issue(&["localhost", "127.0.0.1"], Validity::Current)
}

fn json_response(status: StatusCode, body: serde_json::Value) -> Response {
    (status, axum::Json(body)).into_response()
}

/// An SSE body delivered in small chunks, split mid-line, the way a proxy
/// or a slow upstream would.
fn sse_response(events: &str) -> Response {
    let chunks: Vec<Result<Bytes, Infallible>> = events
        .as_bytes()
        .chunks(7)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(futures::stream::iter(chunks)))
        .expect("SSE response")
}

const MESSAGE_STREAM: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: ping\n\
data: {\"type\":\"ping\"}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"The pod is \"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"OOMKilled.\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

async fn collect_stream(client: &AnthropicClient, prompt: &str) -> Result<String, String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = client.message_stream(prompt, tx).await;
    let mut text = String::new();
    while let Ok(chunk) = rx.try_recv() {
        text.push_str(&chunk);
    }
    result.map(|()| text)
}

// ---------------------------------------------------------------------------
// Helpers for endpoints and bindings
// ---------------------------------------------------------------------------

fn strict() -> EndpointPolicy {
    EndpointPolicy::default()
}

fn resolve(settings: &EndpointSettings, id: EndpointId) -> Endpoint {
    settings.resolve(id, &strict()).expect("valid endpoint")
}

/// A credential saved while `settings` were configured.
fn saved_with(settings: &EndpointSettings, ids: &[EndpointId]) -> CredentialSource {
    CredentialSource::Stored(Some(
        EndpointBinding::capture(settings, ids, &strict()).expect("capture binding"),
    ))
}

fn user_agent() -> String {
    format!("deckwatch/{}", env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------
// Positive: Anthropic Messages API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_messages_stream_request_shape_and_parsing() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/v1/messages",
            post(|| async { sse_response(MESSAGE_STREAM) }),
        ),
    )
    .await;

    let endpoints =
        EndpointSettings::default().with(EndpointId::AnthropicApi, upstream.url("localhost"));
    let source = saved_with(&endpoints, &[EndpointId::AnthropicApi]);
    let client = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &source,
    )
    .expect("key is bound to this endpoint");

    let text = collect_stream(&client, "Why is my pod crashing?")
        .await
        .expect("stream");
    assert_eq!(text, "The pod is OOMKilled.");

    let req = upstream.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/messages");
    assert_eq!(req.header("x-api-key"), "sk-ant-test");
    assert_eq!(req.header("anthropic-version"), "2023-06-01");
    assert_eq!(req.header("content-type"), "application/json");
    assert_eq!(req.header("user-agent"), user_agent());
    let body = req.json();
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 4096);
    assert!(body["model"]
        .as_str()
        .expect("model")
        .starts_with("claude-"));
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "Why is my pod crashing?");
}

#[tokio::test]
async fn anthropic_non_streaming_message_parses_content_blocks() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/gateway/v1/messages",
            post(|| async {
                json_response(
                    StatusCode::OK,
                    serde_json::json!({
                        "id": "msg_1", "type": "message", "role": "assistant",
                        "content": [{"type": "text", "text": "Hello"}, {"type": "text", "text": " there"}],
                        "stop_reason": "end_turn"
                    }),
                )
            }),
        ),
    )
    .await;

    // A base URL with a path prefix (LLM gateway).
    let endpoints = EndpointSettings::default().with(
        EndpointId::AnthropicApi,
        format!("{}/gateway/", upstream.url("localhost")),
    );
    let client = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &saved_with(&endpoints, &[EndpointId::AnthropicApi]),
    )
    .expect("bound");
    assert_eq!(client.message("hi").await.expect("message"), "Hello there");

    let req = upstream.only_request();
    assert_eq!(req.path, "/gateway/v1/messages");
    assert_eq!(req.json()["stream"], false);
}

#[tokio::test]
async fn anthropic_http_errors_map_to_status_and_body() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/v1/messages",
            post(|| async {
                json_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    serde_json::json!({
                        "type": "error",
                        "error": {"type": "rate_limit_error", "message": "Number of requests has exceeded your rate limit"}
                    }),
                )
            }),
        ),
    )
    .await;

    let endpoints =
        EndpointSettings::default().with(EndpointId::AnthropicApi, upstream.url("localhost"));
    let client = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &saved_with(&endpoints, &[EndpointId::AnthropicApi]),
    )
    .expect("bound");

    let err = collect_stream(&client, "hi").await.expect_err("429");
    assert!(err.starts_with("Anthropic API error 429"), "{err}");
    assert!(err.contains("rate_limit_error"), "{err}");
}

#[tokio::test]
async fn anthropic_stream_error_event_is_reported() {
    let pki = pki();
    let events = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n\
event: error\n\
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/v1/messages",
            post(move || async move { sse_response(events) }),
        ),
    )
    .await;

    let endpoints =
        EndpointSettings::default().with(EndpointId::AnthropicApi, upstream.url("localhost"));
    let client = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &saved_with(&endpoints, &[EndpointId::AnthropicApi]),
    )
    .expect("bound");
    assert_eq!(
        collect_stream(&client, "hi")
            .await
            .expect_err("error event"),
        "Overloaded"
    );
}

// ---------------------------------------------------------------------------
// Positive: Vertex AI (token exchange, then streamRawPredict)
// ---------------------------------------------------------------------------

/// A service account key JSON with a fresh RSA key, and the public key
/// (PKCS#1 DER) to verify what deckwatch signs with it.
fn service_account_key() -> (String, Vec<u8>) {
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
    use aws_lc_rs::signature::KeyPair as _;

    let key = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).expect("RSA key");
    let pkcs8: Pkcs8V1Der<'static> = key.as_der().expect("PKCS#8");
    let b64 = base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref());
    let mut pem = String::from("-----BEGIN PRIVATE KEY-----\n");
    for line in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).expect("base64"));
        pem.push('\n');
    }
    pem.push_str("-----END PRIVATE KEY-----\n");
    let sa = serde_json::json!({
        "type": "service_account",
        "project_id": "deckwatch-test",
        "client_email": "deckwatch@deckwatch-test.iam.gserviceaccount.com",
        "private_key": pem,
    });
    (sa.to_string(), key.public_key().as_ref().to_vec())
}

#[tokio::test]
async fn vertex_token_exchange_then_stream_raw_predict() {
    let pki = pki();
    let token_server = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/token",
            post(|| async {
                json_response(
                    StatusCode::OK,
                    serde_json::json!({"access_token": "ya29.mock-token", "expires_in": 3599, "token_type": "Bearer"}),
                )
            }),
        ),
    )
    .await;
    let model_path = "/v1/projects/deckwatch-test/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-20250514:streamRawPredict";
    let api_server = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(model_path, post(|| async { sse_response(MESSAGE_STREAM) })),
    )
    .await;

    let endpoints = EndpointSettings::default()
        .with(
            EndpointId::VertexToken,
            format!("{}/token", token_server.url("localhost")),
        )
        .with(EndpointId::VertexApi, api_server.url("localhost"));
    let source = saved_with(
        &endpoints,
        &[EndpointId::VertexToken, EndpointId::VertexApi],
    );
    let http = trusted();
    let (sa_json, public_key) = service_account_key();

    let access_token = exchange_sa_key_for_token(
        &http.client(Purpose::AiToken),
        &resolve(&endpoints, EndpointId::VertexToken),
        &sa_json,
        &source,
    )
    .await
    .expect("token exchange");
    assert_eq!(access_token, "ya29.mock-token");

    // Token request: form-encoded JWT bearer grant.
    let req = token_server.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/token");
    assert_eq!(
        req.header("content-type"),
        "application/x-www-form-urlencoded"
    );
    assert_eq!(req.header("user-agent"), user_agent());
    let form = req.form();
    assert_eq!(form.len(), 2, "{form:?}");
    assert_eq!(form[0].0, "grant_type");
    assert_eq!(form[0].1, "urn:ietf:params:oauth:grant-type:jwt-bearer");
    assert_eq!(form[1].0, "assertion");
    let jwt = &form[1].1;
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3, "JWT: {jwt}");
    let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header: serde_json::Value =
        serde_json::from_slice(&b64url.decode(parts[0]).expect("header")).expect("header JSON");
    assert_eq!(header["alg"], "RS256");
    let claims: serde_json::Value =
        serde_json::from_slice(&b64url.decode(parts[1]).expect("claims")).expect("claims JSON");
    assert_eq!(
        claims["iss"],
        "deckwatch@deckwatch-test.iam.gserviceaccount.com"
    );
    assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/cloud-platform"
    );
    assert_eq!(
        claims["exp"].as_u64().expect("exp") - claims["iat"].as_u64().expect("iat"),
        3600
    );
    aws_lc_rs::signature::UnparsedPublicKey::new(
        &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
        &public_key,
    )
    .verify(
        format!("{}.{}", parts[0], parts[1]).as_bytes(),
        &b64url.decode(parts[2]).expect("signature"),
    )
    .expect("JWT is signed with the service account key");

    // streamRawPredict with the minted token.
    let client = AnthropicClient::vertex(
        http.client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::VertexApi),
        "deckwatch-test".to_string(),
        "us-east5".to_string(),
        access_token,
        &source,
    )
    .expect("bound");
    let text = collect_stream(&client, "Explain this log")
        .await
        .expect("stream");
    assert_eq!(text, "The pod is OOMKilled.");

    let req = api_server.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, model_path);
    assert_eq!(req.header("authorization"), "Bearer ya29.mock-token");
    assert_eq!(req.header("anthropic-version"), "2023-06-01");
    assert_eq!(req.header("content-type"), "application/json");
    let body = req.json();
    assert!(body.get("model").is_none(), "model is in the URL: {body}");
    assert_eq!(body["anthropic_version"], "2023-06-01");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["content"], "Explain this log");
}

#[tokio::test]
async fn vertex_token_errors_map_to_status_and_body() {
    let pki = pki();
    let token_server = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/token",
            post(|| async {
                json_response(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": "invalid_grant", "error_description": "Invalid JWT Signature."}),
                )
            }),
        ),
    )
    .await;
    let endpoints = EndpointSettings::default().with(
        EndpointId::VertexToken,
        format!("{}/token", token_server.url("localhost")),
    );
    let (sa_json, _) = service_account_key();
    let err = exchange_sa_key_for_token(
        &trusted().client(Purpose::AiToken),
        &resolve(&endpoints, EndpointId::VertexToken),
        &sa_json,
        &saved_with(
            &endpoints,
            &[EndpointId::VertexToken, EndpointId::VertexApi],
        ),
    )
    .await
    .expect_err("400");
    let msg = err.to_string();
    assert!(msg.contains("GCP token exchange error 400"), "{msg}");
    assert!(msg.contains("invalid_grant"), "{msg}");
}

// ---------------------------------------------------------------------------
// Positive: Bedrock InvokeModel (SigV4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bedrock_invoke_is_sigv4_signed() {
    let pki = pki();
    let model_id = "anthropic.claude-3-haiku-20240307-v1:0";
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/model/{model}/invoke",
            post(|| async {
                json_response(
                    StatusCode::OK,
                    serde_json::json!({
                        "id": "msg_bdrk_1", "type": "message", "role": "assistant",
                        "content": [{"type": "text", "text": "Check the memory limit."}],
                        "stop_reason": "end_turn"
                    }),
                )
            }),
        ),
    )
    .await;

    let endpoints =
        EndpointSettings::default().with(EndpointId::Bedrock, upstream.url("localhost"));
    let client = AnthropicClient::bedrock(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::Bedrock),
        "us-east-1".to_string(),
        model_id.to_string(),
        AwsAuth::Static {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("session-token-example".to_string()),
        },
    )
    .expect("loopback endpoint accepts AWS credentials");

    let text = collect_stream(&client, "Why OOM?").await.expect("invoke");
    assert_eq!(text, "Check the memory limit.");

    let req = upstream.only_request();
    assert_eq!(req.method, "POST");
    // The model ID is one path segment, so ':' is percent-encoded.
    assert_eq!(
        req.path,
        "/model/anthropic.claude-3-haiku-20240307-v1%3A0/invoke"
    );
    assert_eq!(req.header("content-type"), "application/json");
    assert_eq!(req.header("accept"), "application/json");
    assert_eq!(req.header("user-agent"), user_agent());

    let amz_date = req.header("x-amz-date");
    assert_eq!(amz_date.len(), 16, "{amz_date}");
    assert!(
        amz_date.ends_with('Z') && amz_date.as_bytes()[8] == b'T',
        "{amz_date}"
    );
    assert_eq!(req.header("x-amz-security-token"), "session-token-example");
    assert_eq!(
        req.header("x-amz-content-sha256"),
        hex(&sha2::Sha256::digest(&req.body))
    );

    let auth = req.header("authorization");
    let expected_scope = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/{}/us-east-1/bedrock/aws4_request, ",
        &amz_date[..8]
    );
    assert!(auth.starts_with(&expected_scope), "{auth}");
    let signed_headers = auth
        .split("SignedHeaders=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .expect("SignedHeaders");
    for header in [
        "host",
        "x-amz-content-sha256",
        "x-amz-date",
        "x-amz-security-token",
    ] {
        assert!(
            signed_headers.split(';').any(|h| h == header),
            "{header} not signed: {signed_headers}"
        );
    }
    let signature = auth.split("Signature=").nth(1).expect("Signature");
    assert_eq!(signature.len(), 64, "{signature}");
    assert!(signature.bytes().all(|b| b.is_ascii_hexdigit()));

    let body = req.json();
    assert_eq!(body["anthropic_version"], "bedrock-2023-05-31");
    assert_eq!(body["max_tokens"], 4096);
    assert!(body.get("model").is_none(), "model is in the URL: {body}");
    assert_eq!(body["messages"][0]["content"], "Why OOM?");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Positive: GitHub (REST compare and smart-HTTP refs with a query string)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn github_compare_through_configured_enterprise_endpoints() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/api/v3/repos/acme/widgets/compare/{range}",
            get(|| async {
                json_response(
                    StatusCode::OK,
                    serde_json::json!({
                        "status": "ahead",
                        "files": [{"filename": "services/api/main.go"}, {"filename": "docs/README.md"}]
                    }),
                )
            }),
        ),
    )
    .await;

    let base = upstream.url("localhost");
    let endpoints = EndpointSettings::default()
        .with(EndpointId::GithubWeb, base.clone())
        .with(EndpointId::GithubApi, format!("{base}/api/v3"));
    let source = saved_with(
        &endpoints,
        &[
            EndpointId::GithubApi,
            EndpointId::GithubWeb,
            EndpointId::GithubRaw,
        ],
    );
    let files = testing::github_changed_files(
        &trusted().client(Purpose::Git),
        &endpoints,
        &strict(),
        &format!("{base}/acme/widgets.git"),
        GitHubToken {
            token: "ghp_test",
            label: "git token 'ghe'",
            source: &source,
        },
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
    )
    .await
    .expect("changed files");
    assert_eq!(files, vec!["services/api/main.go", "docs/README.md"]);

    let req = upstream.only_request();
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.path,
        "/api/v3/repos/acme/widgets/compare/1111111111111111111111111111111111111111...2222222222222222222222222222222222222222"
    );
    assert_eq!(req.header("authorization"), "Bearer ghp_test");
    assert_eq!(req.header("accept"), "application/vnd.github+json");
    assert_eq!(req.header("user-agent"), user_agent());
}

#[tokio::test]
async fn git_smart_http_refs_query_and_basic_auth() {
    let pki = pki();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let advertisement = format!(
        "001e# service=git-upload-pack\n0000\
         0155{sha} HEAD\0multi_ack side-band-64k symref=HEAD:refs/heads/main\n\
         003f{sha} refs/heads/main\n\
         0000"
    );
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/acme/widgets.git/info/refs",
            get(move || async move {
                Response::builder()
                    .header(
                        "content-type",
                        "application/x-git-upload-pack-advertisement",
                    )
                    .body(Body::from(advertisement))
                    .expect("refs response")
            }),
        ),
    )
    .await;

    let repo = format!("{}/acme/widgets.git", upstream.url("localhost"));
    let head = testing::check_remote_head(
        &trusted().client(Purpose::Git),
        &repo,
        "main",
        "ghp_test",
        "x-access-token",
    )
    .await
    .expect("remote head");
    assert_eq!(head, sha);

    let req = upstream.only_request();
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/acme/widgets.git/info/refs");
    assert_eq!(req.query.as_deref(), Some("service=git-upload-pack"));
    let expected = base64::engine::general_purpose::STANDARD.encode("x-access-token:ghp_test");
    assert_eq!(req.header("authorization"), format!("Basic {expected}"));
}

// ---------------------------------------------------------------------------
// Positive: notification webhook, Prometheus, tracing backend
// ---------------------------------------------------------------------------

#[tokio::test]
async fn notification_webhook_posts_slack_compatible_json() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route("/services/T000/B000/XXXX", post(|| async { "ok" })),
    )
    .await;
    let url = format!("{}/services/T000/B000/XXXX", upstream.url("localhost"));
    let payload = testing::build_payload(&testing::NotificationEvent::Test {
        source: "contract test".to_string(),
    });

    testing::post_webhook(&trusted().client(Purpose::Webhook), &url, &payload)
        .await
        .expect("webhook");

    let req = upstream.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.header("content-type"), "application/json");
    let body = req.json();
    assert_eq!(body, payload);
    assert_eq!(body["event_type"], "test");
    assert!(body["text"]
        .as_str()
        .expect("text")
        .contains("contract test"));
    assert!(body["attachments"].is_array());
}

#[tokio::test]
async fn notification_webhook_errors_carry_status_and_body() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/hook",
            post(|| async { (StatusCode::NOT_FOUND, "no_service") }),
        ),
    )
    .await;
    let err = testing::post_webhook(
        &trusted().client(Purpose::Webhook),
        &format!("{}/hook", upstream.url("localhost")),
        &serde_json::json!({"text": "x"}),
    )
    .await
    .expect_err("404");
    let msg = err.to_string();
    assert!(msg.contains("404"), "{msg}");
    assert!(msg.contains("no_service"), "{msg}");
}

#[tokio::test]
async fn prometheus_query_range_request_and_matrix_parsing() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/prometheus/api/v1/query_range",
            post(|| async {
                json_response(
                    StatusCode::OK,
                    serde_json::json!({
                        "status": "success",
                        "data": {
                            "resultType": "matrix",
                            "result": [
                                {"metric": {"pod": "api-1"}, "values": [[1700000000, "0.25"], [1700000060, "0.5"]]},
                                {"metric": {}, "values": [[1700000000, "1"]]}
                            ]
                        }
                    }),
                )
            }),
        ),
    )
    .await;

    let base = format!("{}/prometheus", upstream.url("localhost"));
    let query = "sum by (pod) (rate(container_cpu_usage_seconds_total{namespace=\"team-a\"}[5m]))";
    let series = testing::execute_range(
        &trusted().client(Purpose::Observability),
        &base,
        query,
        1_700_000_000,
        1_700_003_600,
        60,
    )
    .await
    .expect("query_range");
    assert_eq!(series.len(), 2);
    assert_eq!(series[0].pod, "api-1");
    assert_eq!(series[0].points.len(), 2);
    assert_eq!(series[0].points[0].t, 1_700_000_000_000);
    assert!((series[0].points[1].v - 0.5).abs() < f64::EPSILON);
    assert_eq!(series[1].pod, "aggregate");

    // deckwatch POSTs the range query form-encoded (Prometheus accepts GET
    // or POST; POST keeps long PromQL out of URLs and access logs).
    let req = upstream.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/prometheus/api/v1/query_range");
    assert_eq!(
        req.header("content-type"),
        "application/x-www-form-urlencoded"
    );
    let form = req.form();
    assert_eq!(
        form,
        vec![
            ("query".to_string(), query.to_string()),
            ("start".to_string(), "1700000000".to_string()),
            ("end".to_string(), "1700003600".to_string()),
            ("step".to_string(), "60s".to_string()),
        ]
    );
}

#[tokio::test]
async fn tracing_backend_query_parameters() {
    let pki = pki();
    let upstream = MockUpstream::start(
        localhost_cert(&pki.extra),
        Router::new().route(
            "/api/traces",
            get(|| async {
                json_response(
                    StatusCode::OK,
                    serde_json::json!({
                        "data": [{
                            "traceID": "4bf92f3577b34da6a3ce929d0e0e4736",
                            "spans": [
                                {"operationName": "GET /orders", "startTime": 1700000000000000u64, "duration": 25000, "references": []},
                                {"operationName": "SELECT", "startTime": 1700000000005000u64, "duration": 5000, "references": [{"refType": "CHILD_OF"}]}
                            ]
                        }]
                    }),
                )
            }),
        ),
    )
    .await;

    let traces = testing::fetch_jaeger_compat(
        &trusted().client(Purpose::Observability),
        &upstream.url("localhost"),
        "orders-api",
        5,
    )
    .await
    .expect("traces");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(traces[0].root_span_name, "GET /orders");
    assert_eq!(traces[0].span_count, 2);

    let req = upstream.only_request();
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/api/traces");
    assert_eq!(req.query.as_deref(), Some("service=orders-api&limit=5"));
}

// ---------------------------------------------------------------------------
// Negative: TLS trust
// ---------------------------------------------------------------------------

/// Point a native Anthropic client built from `http` at `upstream` and send.
async fn anthropic_call(
    http: &OutboundHttp,
    upstream: &MockUpstream,
    host: &str,
) -> Result<String, String> {
    let endpoints = EndpointSettings::default().with(EndpointId::AnthropicApi, upstream.url(host));
    let client = AnthropicClient::native(
        http.client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &saved_with(&endpoints, &[EndpointId::AnthropicApi]),
    )
    .expect("bound");
    client.message("hi").await
}

/// The error must carry the TLS reason, not just "error sending request".
/// Wording differs per backend (OpenSSL, Security framework, rustls), so
/// accept any of `needles`.
fn assert_tls_reason(err: &str, needles: &[&str]) {
    let lower = err.to_lowercase();
    assert!(
        needles.iter().any(|n| lower.contains(n)),
        "expected one of {needles:?} in: {err}"
    );
}

fn messages_router() -> Router {
    Router::new().route(
        "/v1/messages",
        post(|| async {
            json_response(
                StatusCode::OK,
                serde_json::json!({"content": [{"type": "text", "text": "ok"}]}),
            )
        }),
    )
}

#[tokio::test]
async fn untrusted_ca_without_extra_ca_configured_fails() {
    let pki = pki();
    let upstream = MockUpstream::start(localhost_cert(&pki.extra), messages_router()).await;

    // Same server, trusted through the extra CA: works.
    assert_eq!(
        anthropic_call(&trusted(), &upstream, "localhost")
            .await
            .expect("trusted"),
        "ok"
    );
    // Without the extra CA the test CA is unknown.
    let err = anthropic_call(&without_extra_ca(), &upstream, "localhost")
        .await
        .expect_err("untrusted CA");
    assert!(err.contains("API request failed"), "{err}");
    assert_tls_reason(&err, &["certificate"]);
    assert_eq!(
        upstream.requests().len(),
        1,
        "only the trusted call was served"
    );
}

#[tokio::test]
async fn certificate_from_an_unknown_ca_fails() {
    let pki = pki();
    let upstream = MockUpstream::start(localhost_cert(&pki.rogue), messages_router()).await;
    let err = anthropic_call(&trusted(), &upstream, "localhost")
        .await
        .expect_err("rogue CA");
    assert!(err.contains("API request failed"), "{err}");
    assert_tls_reason(&err, &["certificate"]);
    assert!(upstream.connections() >= 1, "the client did connect");
    assert!(upstream.requests().is_empty(), "no request may be served");
}

#[tokio::test]
async fn hostname_mismatch_fails() {
    let pki = pki();
    let cert = pki
        .extra
        .issue(&["upstream.deckwatch.test"], Validity::Current);
    let upstream = MockUpstream::start(cert, messages_router()).await;
    let err = anthropic_call(&trusted(), &upstream, "localhost")
        .await
        .expect_err("hostname mismatch");
    assert!(err.contains("API request failed"), "{err}");
    assert_tls_reason(
        &err,
        &[
            "host name",
            "hostname",
            "not valid for name",
            "notvalidforname",
        ],
    );
    assert!(upstream.connections() >= 1);
    assert!(upstream.requests().is_empty());
}

#[tokio::test]
async fn expired_certificate_fails() {
    let pki = pki();
    let cert = pki
        .extra
        .issue(&["localhost", "127.0.0.1"], Validity::Expired);
    let upstream = MockUpstream::start(cert, messages_router()).await;
    let err = anthropic_call(&trusted(), &upstream, "localhost")
        .await
        .expect_err("expired");
    assert!(err.contains("API request failed"), "{err}");
    assert_tls_reason(&err, &["expired"]);
    assert!(upstream.connections() >= 1);
    assert!(upstream.requests().is_empty());
}

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "SSL_CERT_FILE stands in for the system store only on Linux"
)]
async fn extra_ca_is_added_to_the_system_roots_not_instead_of_them() {
    let pki = pki();
    let system_signed = MockUpstream::start(localhost_cert(&pki.system), messages_router()).await;
    let extra_signed = MockUpstream::start(localhost_cert(&pki.extra), messages_router()).await;
    let http = trusted();

    assert_eq!(
        anthropic_call(&http, &system_signed, "localhost")
            .await
            .expect("system root still trusted with an extra CA configured"),
        "ok"
    );
    assert_eq!(
        anthropic_call(&http, &extra_signed, "localhost")
            .await
            .expect("extra CA trusted"),
        "ok"
    );
    // Control: the system root alone does not trust the extra CA.
    assert!(
        anthropic_call(&without_extra_ca(), &extra_signed, "localhost")
            .await
            .is_err()
    );
    assert_eq!(
        anthropic_call(&without_extra_ca(), &system_signed, "localhost")
            .await
            .expect("system root"),
        "ok"
    );
}

// ---------------------------------------------------------------------------
// Negative: credential binding (refused before anything is sent)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_key_bound_to_host_a_is_not_sent_to_host_b() {
    let pki = pki();
    let host_a = MockUpstream::start(localhost_cert(&pki.extra), messages_router()).await;
    let host_b = MockUpstream::start(localhost_cert(&pki.extra), messages_router()).await;

    let saved_for_a =
        EndpointSettings::default().with(EndpointId::AnthropicApi, host_a.url("localhost"));
    let source = saved_with(&saved_for_a, &[EndpointId::AnthropicApi]);
    let now_b = EndpointSettings::default().with(EndpointId::AnthropicApi, host_b.url("localhost"));

    let err = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&now_b, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &source,
    )
    .err()
    .expect("refused");
    assert!(
        matches!(err, ClientError::Binding(BindingError::HostMismatch { .. })),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("localhost:{}", host_a.addr.port())),
        "{msg}"
    );
    assert!(
        msg.contains(&format!("localhost:{}", host_b.addr.port())),
        "{msg}"
    );
    assert!(msg.contains("re-enter"), "{msg}");

    assert_eq!(host_a.connections(), 0);
    assert_eq!(host_b.connections(), 0, "nothing may reach host B");
}

#[tokio::test]
async fn legacy_key_is_not_sent_to_a_custom_host() {
    let pki = pki();
    let custom = MockUpstream::start(localhost_cert(&pki.extra), messages_router()).await;
    let endpoints =
        EndpointSettings::default().with(EndpointId::AnthropicApi, custom.url("localhost"));
    // Stored before endpoint settings existed: bound to api.anthropic.com.
    let err = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &CredentialSource::Stored(None),
    )
    .err()
    .expect("refused");
    assert!(err.to_string().contains("api.anthropic.com"), "{err}");
    // Same for a key from a Kubernetes Secret.
    let err = AnthropicClient::native(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::AnthropicApi),
        "sk-ant-test".to_string(),
        &CredentialSource::KubernetesSecret("deckwatch-anthropic-api-key".to_string()),
    )
    .err()
    .expect("refused");
    assert!(
        matches!(
            err,
            ClientError::Binding(BindingError::SecretOnlyForDefault { .. })
        ),
        "{err:?}"
    );
    assert_eq!(custom.connections(), 0);
}

#[tokio::test]
async fn gcp_key_bound_to_token_host_a_is_not_used_with_host_b() {
    let pki = pki();
    let token_a = MockUpstream::start(localhost_cert(&pki.extra), Router::new()).await;
    let token_b = MockUpstream::start(localhost_cert(&pki.extra), Router::new()).await;

    let saved_for_a = EndpointSettings::default().with(
        EndpointId::VertexToken,
        format!("{}/token", token_a.url("localhost")),
    );
    let source = saved_with(
        &saved_for_a,
        &[EndpointId::VertexToken, EndpointId::VertexApi],
    );
    let now_b = EndpointSettings::default().with(
        EndpointId::VertexToken,
        format!("{}/token", token_b.url("localhost")),
    );
    let (sa_json, _) = service_account_key();

    let err = exchange_sa_key_for_token(
        &trusted().client(Purpose::AiToken),
        &resolve(&now_b, EndpointId::VertexToken),
        &sa_json,
        &source,
    )
    .await
    .expect_err("refused");
    assert!(matches!(err, ClientError::Binding(_)), "{err:?}");
    assert_eq!(token_a.connections(), 0);
    assert_eq!(token_b.connections(), 0, "no JWT may reach host B");

    // The Vertex API host is bound too: a token minted for the bound hosts
    // is not sent to a different API host.
    let api_b = MockUpstream::start(localhost_cert(&pki.extra), Router::new()).await;
    let api_now_b = EndpointSettings::default().with(EndpointId::VertexApi, api_b.url("localhost"));
    let err = AnthropicClient::vertex(
        trusted().client(Purpose::AiInference),
        &resolve(&api_now_b, EndpointId::VertexApi),
        "deckwatch-test".to_string(),
        "us-east5".to_string(),
        "ya29.token".to_string(),
        &source,
    )
    .err()
    .expect("refused");
    assert!(matches!(err, ClientError::Binding(_)), "{err:?}");
    assert_eq!(api_b.connections(), 0);
}

#[tokio::test]
async fn git_token_bound_to_host_a_is_not_sent_to_host_b() {
    let pki = pki();
    let ghe_a = MockUpstream::start(localhost_cert(&pki.extra), Router::new()).await;
    let ghe_b = MockUpstream::start(localhost_cert(&pki.extra), Router::new()).await;

    let a = ghe_a.url("localhost");
    let saved_for_a = EndpointSettings::default()
        .with(EndpointId::GithubWeb, a.clone())
        .with(EndpointId::GithubApi, format!("{a}/api/v3"));
    let source = saved_with(
        &saved_for_a,
        &[
            EndpointId::GithubApi,
            EndpointId::GithubWeb,
            EndpointId::GithubRaw,
        ],
    );
    // Repositories still live on A's web host; only the API moved to B.
    let now_b = EndpointSettings::default()
        .with(EndpointId::GithubWeb, a.clone())
        .with(
            EndpointId::GithubApi,
            format!("{}/api/v3", ghe_b.url("localhost")),
        );

    let files = testing::github_changed_files(
        &trusted().client(Purpose::Git),
        &now_b,
        &strict(),
        &format!("{a}/acme/widgets.git"),
        GitHubToken {
            token: "ghp_test",
            label: "git token 'ghe'",
            source: &source,
        },
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
    )
    .await;
    assert_eq!(files, None, "path filtering is skipped, the token is kept");
    assert_eq!(ghe_a.connections(), 0);
    assert_eq!(ghe_b.connections(), 0, "the token may not reach host B");
}

#[tokio::test]
async fn aws_credentials_are_not_sent_to_a_non_aws_host() {
    pki();
    let endpoints =
        EndpointSettings::default().with(EndpointId::Bedrock, "https://bedrock.attacker.example");
    let err = AnthropicClient::bedrock(
        trusted().client(Purpose::AiInference),
        &resolve(&endpoints, EndpointId::Bedrock),
        "us-east-1".to_string(),
        String::new(),
        AwsAuth::Static {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "secret".to_string(),
            session_token: None,
        },
    )
    .err()
    .expect("refused");
    assert!(
        matches!(err, ClientError::Binding(BindingError::NotAwsHost { .. })),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// Negative: non-https endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_https_endpoint_is_rejected_without_the_insecure_flag() {
    pki();
    let settings = EndpointSettings::default()
        .with(EndpointId::AnthropicApi, "http://llm-gateway.example.test");

    // Rejected when saved...
    let errors = settings.validated(&strict()).expect_err("rejected on save");
    assert!(matches!(errors.0[0], EndpointError::Insecure { .. }));
    // ...and when used, so a value stored under a looser policy is not
    // dialled either.
    assert!(matches!(
        settings.resolve(EndpointId::AnthropicApi, &trusted().policy()),
        Err(EndpointError::Insecure { .. })
    ));

    // DECKWATCH_ALLOW_INSECURE_ENDPOINTS=true (local dev and tests) lifts it.
    let insecure = EndpointPolicy {
        allow_insecure: true,
    };
    assert!(settings.validated(&insecure).is_ok());
    // Loopback http is always allowed.
    let local = EndpointSettings::default().with(EndpointId::AnthropicApi, "http://127.0.0.1:8080");
    assert!(local.validated(&strict()).is_ok());
}
