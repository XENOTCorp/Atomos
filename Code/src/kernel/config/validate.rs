//! Config checks after parse.
use std::net::IpAddr;
use super::Config;
use crate::error::ServeError;

pub(crate) fn engine_ok(s: &str) -> bool {
    matches!(s, "epoll" | "tokio" | "xdp" | "af-xdp")
}
pub(crate) fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback(),
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), ServeError> {
        if self.bind.is_empty() {
            return Err(ServeError::Config("empty bind".into()));
        }
        let addr: std::net::SocketAddr = self
            .bind
            .parse()
            .map_err(|_| ServeError::Config("bind is not host:port".into()))?;
        if !self.allow_non_loopback && !is_loopback(addr.ip()) {
            return Err(ServeError::Config("non-loopback bind refused".into()));
        }
        if self.workers == 0 {
            return Err(ServeError::Config("workers == 0".into()));
        }
        if self.max_json_depth == 0 || self.max_json_depth > 64 {
            return Err(ServeError::Config("max_json_depth not in 1..=64".into()));
        }
        if self.max_body_bytes < 1024 {
            return Err(ServeError::Config("max_body_bytes < 1024".into()));
        }
        if self.memory_cap_bytes < 16 * 1024 * 1024 {
            return Err(ServeError::Config("memory_cap_bytes < 16MiB".into()));
        }
        if !(self.cpu_fraction > 0.0 && self.cpu_fraction <= 1.0) {
            return Err(ServeError::Config("cpu_fraction not in (0,1]".into()));
        }
        match (&self.tls_cert, &self.tls_key) {
            (None, None) | (Some(_), Some(_)) => {}
            _ => {
                return Err(ServeError::Config(
                    "tls_cert and tls_key must both be set".into(),
                ));
            }
        }
        if !engine_ok(&self.engine) {
            return Err(ServeError::Config("unknown engine".into()));
        }
        if self.engine == "epoll" && (self.http2 || self.http3) {
            return Err(ServeError::Config(
                "engine=epoll cannot set http2/http3 (I_engine; use atomos-proto)".into(),
            ));
        }
        if self.engine == "xdp" || self.engine == "af-xdp" {
            return Err(ServeError::Config(
                "engine xdp is not linked in this site".into(),
            ));
        }
        Ok(())
    }

    pub fn port(&self) -> Result<u16, ServeError> {
        let addr: std::net::SocketAddr = self
            .bind
            .parse()
            .map_err(|_| ServeError::Config("bind is not host:port".into()))?;
        Ok(addr.port())
    }
}
