//! Extra trusted CA certificates (`DECKWATCH_EXTRA_CA_FILE`).
//!
//! The bundle is a PEM file with one or more `CERTIFICATE` blocks. Every
//! outbound client trusts these certificates *in addition to* the system
//! roots; nothing here can remove or replace a system root. See
//! `docs/TLS.md`.
//!
//! Parsing is TLS-backend neutral: certificates are decoded with
//! `rustls-pki-types` and validated as trust anchors with webpki (through
//! `rustls::RootCertStore`) before they are handed to reqwest, so a broken
//! bundle is rejected with the same message on native-tls (reqwest 0.12) and
//! rustls (reqwest 0.13).

use std::fmt;
use std::path::{Path, PathBuf};

use rustls::pki_types::pem::{PemObject, SectionKind};
use rustls::pki_types::CertificateDer;

/// Why an extra CA bundle was rejected. Every variant names the file, so the
/// startup error points straight at the misconfigured mount.
#[derive(Debug, thiserror::Error)]
pub enum ExtraCaError {
    #[error("cannot read extra CA bundle {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("extra CA bundle {} is not valid PEM (block {index}): {detail}", path.display())]
    Pem {
        path: PathBuf,
        index: usize,
        detail: String,
    },
    #[error(
        "extra CA bundle {} contains a private key (block {index}); \
         it must contain only CA certificates",
        path.display()
    )]
    PrivateKey { path: PathBuf, index: usize },
    #[error("extra CA bundle {} contains no PEM CERTIFICATE blocks", path.display())]
    Empty { path: PathBuf },
    #[error(
        "extra CA bundle {}: certificate {index} is not a usable trust anchor: {detail}",
        path.display()
    )]
    Certificate {
        path: PathBuf,
        index: usize,
        detail: String,
    },
    #[error(
        "extra CA bundle {}: the TLS backend rejected certificate {index}: {detail}",
        path.display()
    )]
    Backend {
        path: PathBuf,
        index: usize,
        detail: String,
    },
}

/// A validated set of extra CA certificates and the file they came from.
#[derive(Clone)]
pub struct ExtraCaBundle {
    path: PathBuf,
    certs: Vec<CertificateDer<'static>>,
}

impl fmt::Debug for ExtraCaBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtraCaBundle")
            .field("path", &self.path)
            .field("certificates", &self.certs.len())
            .finish()
    }
}

impl ExtraCaBundle {
    /// Read and validate a PEM bundle from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ExtraCaError> {
        let path = path.as_ref();
        let pem = std::fs::read(path).map_err(|source| ExtraCaError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_pem(path, &pem)
    }

    /// Validate PEM bytes. `path` is only used in error messages and
    /// [`Self::path`].
    pub fn from_pem(path: impl AsRef<Path>, pem: &[u8]) -> Result<Self, ExtraCaError> {
        let path = path.as_ref().to_path_buf();
        let mut certs = Vec::new();
        for (i, section) in <(SectionKind, Vec<u8>)>::pem_slice_iter(pem).enumerate() {
            let index = i + 1;
            let (kind, der) = section.map_err(|e| ExtraCaError::Pem {
                path: path.clone(),
                index,
                detail: e.to_string(),
            })?;
            match kind {
                SectionKind::Certificate => {
                    let cert = CertificateDer::from(der);
                    // webpki parses the certificate the way a TLS client will
                    // when it uses it as a root. Catches truncated or non-X.509
                    // blocks before any client is built.
                    let mut probe = rustls::RootCertStore::empty();
                    probe
                        .add(cert.clone())
                        .map_err(|e| ExtraCaError::Certificate {
                            path: path.clone(),
                            index: certs.len() + 1,
                            detail: e.to_string(),
                        })?;
                    certs.push(cert);
                }
                SectionKind::PrivateKey
                | SectionKind::RsaPrivateKey
                | SectionKind::EcPrivateKey => {
                    return Err(ExtraCaError::PrivateKey { path, index });
                }
                // CRLs, CSRs and public keys carry no trust; ignore them like
                // OpenSSL does when loading a CA file.
                _ => {}
            }
        }
        if certs.is_empty() {
            return Err(ExtraCaError::Empty { path });
        }
        Ok(Self { path, certs })
    }

    /// File the bundle was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// DER-encoded certificates, in file order.
    pub fn certificates(&self) -> &[CertificateDer<'static>] {
        &self.certs
    }

    pub fn len(&self) -> usize {
        self.certs.len()
    }

    /// Always false: an empty bundle is rejected by [`Self::from_pem`].
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }

    /// The certificates as reqwest roots. Fails if the active TLS backend
    /// cannot parse one of them (native-tls parses eagerly).
    pub(crate) fn reqwest_certificates(&self) -> Result<Vec<reqwest::Certificate>, ExtraCaError> {
        self.certs
            .iter()
            .enumerate()
            .map(|(i, der)| {
                reqwest::Certificate::from_der(der.as_ref()).map_err(|e| ExtraCaError::Backend {
                    path: self.path.clone(),
                    index: i + 1,
                    detail: e.to_string(),
                })
            })
            .collect()
    }

    /// The certificates as `object_store` roots (S3 registry backend, AWS
    /// credential providers).
    pub(crate) fn object_store_certificates(
        &self,
    ) -> Result<Vec<object_store::Certificate>, ExtraCaError> {
        self.certs
            .iter()
            .enumerate()
            .map(|(i, der)| {
                object_store::Certificate::from_der(der.as_ref()).map_err(|e| {
                    ExtraCaError::Backend {
                        path: self.path.clone(),
                        index: i + 1,
                        detail: e.to_string(),
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
