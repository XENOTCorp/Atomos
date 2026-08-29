//! rustls configs for TCP (HTTP/1.1 + h2) and QUIC (h3). Self-signed if no PEM.

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once, OnceLock};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ProducesTickets;
use rustls::ServerConfig as RustlsServer;

use crate::error::ServeError;

pub struct TlsSet {
    pub tcp: Arc<RustlsServer>,
    pub quic: quinn::ServerConfig,
}

/// Load rustls/QUIC once, on first TLS or HTTP/3 use. h2c does not touch this.
pub struct TlsHold {
    inner: OnceLock<Arc<TlsSet>>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    ocsp: Option<PathBuf>,
    /// Stored for proto / callers; also applied when `get` builds configs.
    ticket_lifetime_secs: u64,
}

impl TlsHold {
    pub fn with_opts(
        cert: Option<PathBuf>,
        key: Option<PathBuf>,
        ocsp: Option<PathBuf>,
        ticket_lifetime_secs: u64,
    ) -> Self {
        Self {
            inner: OnceLock::new(),
            cert,
            key,
            ocsp,
            ticket_lifetime_secs,
        }
    }

    pub fn get(&self) -> Result<Arc<TlsSet>, ServeError> {
        if let Some(t) = self.inner.get() {
            return Ok(t.clone());
        }
        let ocsp_bytes = match &self.ocsp {
            Some(p) => Some(std::fs::read(p).map_err(ServeError::from)?),
            None => None,
        };
        let loaded = Arc::new(load_with_opts(
            self.cert.as_deref(),
            self.key.as_deref(),
            ocsp_bytes.as_deref(),
            self.ticket_lifetime_secs,
        )?);
        match self.inner.set(loaded.clone()) {
            Ok(()) => Ok(loaded),
            Err(_) => Ok(self.inner.get().expect("tls").clone()),
        }
    }
}

fn install_ring() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn load(cert: Option<&Path>, key: Option<&Path>) -> Result<TlsSet, ServeError> {
    load_with_opts(cert, key, None, 86400)
}

/// Epoll H1: same cert loader as proto, ALPN restricted to `http/1.1`.
/// Clients that offer only `h2` fail the handshake. No H2 on this path.
pub fn h1_only_server(
    cert: Option<&Path>,
    key: Option<&Path>,
    ocsp: Option<&[u8]>,
    ticket_lifetime_secs: u64,
) -> Result<Arc<RustlsServer>, ServeError> {
    let set = load_with_opts(cert, key, ocsp, ticket_lifetime_secs)?;
    let mut cfg = (*set.tcp).clone();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

pub fn load_with_opts(
    cert: Option<&Path>,
    key: Option<&Path>,
    ocsp: Option<&[u8]>,
    ticket_lifetime_secs: u64,
) -> Result<TlsSet, ServeError> {
    install_ring();
    let (certs, pk) = match (cert, key) {
        (Some(c), Some(k)) => load_pem(c, k)?,
        (None, None) => self_signed()?,
        _ => {
            return Err(ServeError::Config(
                "tls_cert and tls_key must both be set".into(),
            ));
        }
    };
    let ticketer = make_ticketer(ticket_lifetime_secs)?;
    let tcp = rustls_server(
        certs.clone(),
        pk.clone_key(),
        &[b"h2", b"http/1.1"],
        ocsp,
        Arc::clone(&ticketer),
    )?;
    let mut h3 = rustls_server(certs, pk, &[b"h3"], ocsp, ticketer)?;
    h3.max_early_data_size = u32::MAX;
    let quic_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(h3)
        .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
    let mut quic = quinn::ServerConfig::with_crypto(Arc::new(quic_crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(256u32.into());
    quic.transport_config(Arc::new(transport));
    Ok(TlsSet {
        tcp: Arc::new(tcp),
        quic,
    })
}

fn make_ticketer(lifetime_secs: u64) -> Result<Arc<dyn ProducesTickets>, ServeError> {
    let inner = rustls::crypto::ring::Ticketer::new()
        .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
    if lifetime_secs == 0 {
        return Ok(inner);
    }
    let lifetime = u32::try_from(lifetime_secs).unwrap_or(u32::MAX);
    Ok(Arc::new(LifetimeTickets { inner, lifetime }))
}

/// Advertises `lifetime` while delegating encrypt/decrypt to ring's rotating ticketer.
struct LifetimeTickets {
    inner: Arc<dyn ProducesTickets>,
    lifetime: u32,
}

impl fmt::Debug for LifetimeTickets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LifetimeTickets")
            .field("lifetime", &self.lifetime)
            .finish_non_exhaustive()
    }
}

impl ProducesTickets for LifetimeTickets {
    fn enabled(&self) -> bool {
        self.inner.enabled()
    }

    fn lifetime(&self) -> u32 {
        self.lifetime
    }

    fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
        self.inner.encrypt(plain)
    }

    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        self.inner.decrypt(cipher)
    }
}

fn rustls_server(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    alpn: &[&[u8]],
    ocsp: Option<&[u8]>,
    ticketer: Arc<dyn ProducesTickets>,
) -> Result<RustlsServer, ServeError> {
    let builder = RustlsServer::builder().with_no_client_auth();
    let mut cfg = match ocsp {
        Some(o) => builder.with_single_cert_with_ocsp(certs, key, o.to_vec()),
        None => builder.with_single_cert(certs, key),
    }
    .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    cfg.ticketer = ticketer;
    Ok(cfg)
}

fn self_signed() -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ServeError> {
    let ck = rcgen::generate_simple_self_signed(vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "::1".into(),
    ])
    .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
    let cert = CertificateDer::from(ck.cert);
    let key = PrivateKeyDer::Pkcs8(ck.key_pair.serialize_der().into());
    Ok((vec![cert], key))
}

fn load_pem(
    cert: &Path,
    key: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ServeError> {
    let mut cr = BufReader::new(File::open(cert)?);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cr)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
    if certs.is_empty() {
        return Err(ServeError::Config("tls_cert has no certificates".into()));
    }
    let mut kr = BufReader::new(File::open(key)?);
    let pk = rustls_pemfile::private_key(&mut kr)
        .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?
        .ok_or_else(|| ServeError::Config("tls_key has no private key".into()))?;
    Ok((certs, pk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_signed_builds_tcp_and_quic() {
        let t = load(None, None).expect("tls");
        assert!(!t.tcp.alpn_protocols.is_empty());
    }

    #[test]
    fn load_with_opts_none_ocsp_matches_load() {
        let a = load(None, None).expect("load");
        let b = load_with_opts(None, None, None, 86400).expect("load_with_opts");
        assert_eq!(a.tcp.alpn_protocols, b.tcp.alpn_protocols);
        assert_eq!(a.tcp.ticketer.lifetime(), b.tcp.ticketer.lifetime());
        assert!(a.tcp.ticketer.enabled());
        assert!(b.tcp.ticketer.enabled());
    }

    #[test]
    fn load_with_opts_accepts_ocsp_bytes() {
        let t = load_with_opts(None, None, Some(&[0u8; 8]), 86400).expect("ocsp");
        assert!(!t.tcp.alpn_protocols.is_empty());
        assert_eq!(t.tcp.ticketer.lifetime(), 86400);
    }

    #[test]
    fn h1_only_alpn_is_http11() {
        let c = h1_only_server(None, None, None, 86400).expect("h1");
        assert_eq!(c.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }
}
