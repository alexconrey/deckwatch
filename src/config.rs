use clap::Parser;

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
