use std::time::Duration;

use clap::Parser;

/// The environment variable that sets [`Config::plugin_call_timeout`].
const PLUGIN_CALL_TIMEOUT_ENV: &str = "DECKWATCH_PLUGIN_CALL_TIMEOUT";

/// The longest plugin call time limit deckwatch accepts, in seconds. Plugin
/// calls hold up the request, provision or reconcile pass that made them, so
/// a limit anywhere near an hour isn't useful, and a bound keeps the value
/// well inside what extism handles (it converts the limit to milliseconds in
/// a `u64` and adds it to an `Instant`).
const MAX_PLUGIN_CALL_TIMEOUT_SECS: u64 = 3600;

#[derive(Parser, Debug)]
#[command(name = "deckwatch", about = "Kubernetes deployment lifecycle manager")]
pub struct Config {
    /// Comma-separated list of namespaces to manage. Empty = all namespaces.
    #[arg(
        long,
        env = "DECKWATCH_NAMESPACES",
        value_delimiter = ',',
        default_value = ""
    )]
    pub namespaces: Vec<String>,

    /// Port to listen on.
    #[arg(long, env = "DECKWATCH_PORT", default_value = "8080")]
    pub port: u16,

    /// Path to frontend dist directory.
    #[arg(long, env = "DECKWATCH_FRONTEND_DIR", default_value = "frontend/dist")]
    pub frontend_dir: String,

    /// Path to the rendered mdBook site (`docs/book/book/` after
    /// `scripts/build-docs.sh`). Served at `/docs/book/` — missing directory
    /// simply means the operator manual link 404s, the rest of the app
    /// works fine.
    #[arg(long, env = "DECKWATCH_BOOK_DIR", default_value = "docs/book/book")]
    pub book_dir: String,

    #[arg(long, env = "DECKWATCH_SETTINGS_NAMESPACE")]
    pub settings_namespace: Option<String>,

    #[arg(
        long,
        env = "DECKWATCH_SETTINGS_CONFIGMAP",
        default_value = "deckwatch-config"
    )]
    pub settings_configmap_name: String,

    /// Enable the embedded OCI registry. When set, the `/v2/*` endpoints
    /// (Distribution Spec v1.1) and the `/api/registry/*` UI API are mounted.
    /// Kaniko builds triggered by GitOps will push here when they resolve
    /// to `<registry_service>:5000/...`.
    #[arg(long, env = "DECKWATCH_REGISTRY_ENABLED", default_value = "false")]
    pub registry_enabled: bool,

    /// Storage backend for the embedded registry: `filesystem` (default) or
    /// `s3`. Filesystem needs a PVC mounted at `registry_root`; S3 needs
    /// the `registry_s3_*` fields below plus AWS credentials from the
    /// standard chain (env / IRSA / IMDS).
    #[arg(long, env = "DECKWATCH_REGISTRY_STORAGE", default_value = "filesystem")]
    pub registry_storage: String,

    /// On-disk root for the embedded OCI registry. Only read when
    /// `registry_enabled` is true and `registry_storage == "filesystem"`.
    /// Should be a PVC mount in production so blobs survive pod restarts.
    #[arg(
        long,
        env = "DECKWATCH_REGISTRY_ROOT",
        default_value = "/data/registry"
    )]
    pub registry_root: String,

    /// S3 bucket for the embedded registry. Required when
    /// `registry_storage == "s3"`.
    #[arg(long, env = "DECKWATCH_REGISTRY_S3_BUCKET", default_value = "")]
    pub registry_s3_bucket: String,

    /// Optional key prefix so multiple deckwatch instances can share one
    /// bucket. Empty = bucket root.
    #[arg(long, env = "DECKWATCH_REGISTRY_S3_PREFIX", default_value = "")]
    pub registry_s3_prefix: String,

    /// AWS region for the S3 bucket. Ignored by MinIO / R2 (they use the
    /// endpoint), but the SDK still needs a value.
    #[arg(
        long,
        env = "DECKWATCH_REGISTRY_S3_REGION",
        default_value = "us-east-1"
    )]
    pub registry_s3_region: String,

    /// Custom S3 endpoint for MinIO / Ceph RGW / Cloudflare R2. Empty =
    /// AWS default. Setting an endpoint auto-enables path-style addressing.
    #[arg(long, env = "DECKWATCH_REGISTRY_S3_ENDPOINT", default_value = "")]
    pub registry_s3_endpoint: String,

    /// Force path-style S3 addressing when talking to AWS itself. Ignored
    /// (already true) when a custom endpoint is set.
    #[arg(
        long,
        env = "DECKWATCH_REGISTRY_S3_PATH_STYLE",
        default_value = "false"
    )]
    pub registry_s3_path_style: bool,

    /// Public URL the registry is reachable at, used to populate the
    /// auto-generated "Deckwatch Registry (local)" entry in the OCI
    /// Registries settings list. Typically
    /// `deckwatch-registry.<namespace>.svc.cluster.local:5000` for
    /// in-cluster kaniko builds.
    #[arg(long, env = "DECKWATCH_REGISTRY_PUBLIC_URL", default_value = "")]
    pub registry_public_url: String,

    /// In-cluster registry URL that Kaniko uses to push built images.
    /// Defaults to the same value as `registry_public_url`. Set this when
    /// the kubelet pull URL differs from the in-cluster push URL (e.g.
    /// k3d NodePort setups where push goes to the ClusterIP Service but
    /// pull goes through localhost:nodePort).
    #[arg(long, env = "DECKWATCH_REGISTRY_INTERNAL_URL", default_value = "")]
    pub registry_internal_url: String,

    /// Database URL. Defaults to SQLite file.
    /// Examples:
    ///   sqlite:///data/deckwatch.db?mode=rwc
    ///   postgres://user:pass@host:5432/deckwatch
    ///   mysql://user:pass@host:3306/deckwatch
    #[arg(
        long,
        env = "DECKWATCH_DATABASE_URL",
        default_value = "sqlite:///app/deckwatch.db?mode=rwc"
    )]
    pub database_url: String,

    /// Encryption key for API keys stored in the database. When set, the
    /// settings credentials endpoint encrypts values with AES-256-GCM before
    /// persisting them. The key is derived via SHA-256, so any non-empty
    /// string works. In production, Helm generates a random Secret on first
    /// install (`secret-encryption-key.yaml`).
    #[arg(long, env = "DECKWATCH_ENCRYPTION_KEY", default_value = "")]
    pub encryption_key: String,

    /// Days to keep audit log rows; older rows are pruned hourly. 0 keeps
    /// them regardless of age. Deployment config rather than a setting, so
    /// the unauthenticated settings API can't shorten the audit trail.
    #[arg(long, env = "DECKWATCH_AUDIT_RETENTION_DAYS", default_value = "365")]
    pub audit_retention_days: u32,

    /// The most audit log rows to keep, newest first; older ones are pruned
    /// hourly. 0 means no limit.
    #[arg(long, env = "DECKWATCH_AUDIT_MAX_ROWS", default_value = "100000")]
    pub audit_max_rows: u64,

    /// Directory for deckwatch's cache: plugin calls cache compiled
    /// WebAssembly in `<dir>/wasmtime`. Unset or empty means no cache, and so
    /// does a directory deckwatch can't use (with a warning).
    // An OsString, not a PathBuf: clap refuses an empty PathBuf, which would
    // stop deckwatch from starting. See src/plugin_cache.rs.
    #[arg(long, env = "DECKWATCH_CACHE_DIR")]
    pub cache_dir: Option<std::ffi::OsString>,

    /// The longest one plugin call may run, in whole seconds, from 1 to 3600.
    /// A call that runs longer is stopped and fails with `plugin.timeout`; the
    /// plugin's HTTP requests count toward it. Plugin calls always have a time
    /// limit: deckwatch doesn't start with 0, an empty value or anything else
    /// outside that range.
    #[arg(
        long,
        env = PLUGIN_CALL_TIMEOUT_ENV,
        value_name = "SECONDS",
        default_value = "60",
        value_parser = parse_plugin_call_timeout
    )]
    pub plugin_call_timeout: Duration,
}

/// Parse a plugin call time limit: whole seconds, 1 to
/// [`MAX_PLUGIN_CALL_TIMEOUT_SECS`].
///
/// 0 doesn't mean "no limit", as it does for the audit limits: that would
/// bring back #301. extism gives a plugin's HTTP request what's left of the
/// call's time limit as its only timeout, so without a limit, a request that
/// never gets an answer holds its thread for good. Empty is refused like any
/// other value that isn't a whole number.
fn parse_plugin_call_timeout(value: &str) -> Result<Duration, String> {
    match value.trim().parse::<u64>() {
        Ok(0) => Err(format!(
            "{PLUGIN_CALL_TIMEOUT_ENV} can't be 0: plugin calls always have a time limit, \
             from 1 to {MAX_PLUGIN_CALL_TIMEOUT_SECS} seconds"
        )),
        Ok(secs) if secs <= MAX_PLUGIN_CALL_TIMEOUT_SECS => Ok(Duration::from_secs(secs)),
        _ => Err(format!(
            "{PLUGIN_CALL_TIMEOUT_ENV} must be a whole number of seconds, \
             from 1 to {MAX_PLUGIN_CALL_TIMEOUT_SECS}"
        )),
    }
}

impl Config {
    pub fn allowed_namespaces(&self) -> Vec<String> {
        self.namespaces
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
