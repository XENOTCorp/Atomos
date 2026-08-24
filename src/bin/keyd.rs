//! atomos-keyd — private keys out of workers. Single-threaded Unix sign daemon.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atomos::jail;
use atomos::ops::keyproto::{self, KIND_SIGN, MAX_FRAME};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::sign::SigningKey;
use rustls::SignatureScheme;

const IO_TIMEOUT: Duration = Duration::from_secs(5);

const OFFERED: &[SignatureScheme] = &[
    SignatureScheme::ECDSA_NISTP256_SHA256,
    SignatureScheme::ECDSA_NISTP384_SHA384,
    SignatureScheme::ED25519,
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::RSA_PKCS1_SHA256,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA512,
];

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "atomos-keyd --key PEM [--sock PATH]\nDefault sock $XDG_RUNTIME_DIR/atomos-keyd.sock else /tmp/atomos-keyd.sock."
        );
        std::process::exit(0);
    }
    if let Err(e) = run(&args) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (key_path, sock_path) = parse_args(args)?;
    let key = load_signing_key(&key_path)?;
    serve(&sock_path, key.as_ref())
}

fn parse_args(args: &[String]) -> Result<(PathBuf, PathBuf), String> {
    let mut key = None;
    let mut sock = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--key" => {
                i += 1;
                let p = args.get(i).ok_or("--key needs a path")?;
                key = Some(PathBuf::from(p));
            }
            "--sock" => {
                i += 1;
                let p = args.get(i).ok_or("--sock needs a path")?;
                sock = Some(PathBuf::from(p));
            }
            other => return Err(format!("unknown arg {other}")),
        }
        i += 1;
    }
    Ok((
        key.ok_or("--key PEM is required")?,
        sock.unwrap_or_else(default_sock),
    ))
}

fn default_sock() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("atomos-keyd.sock"),
        _ => PathBuf::from("/tmp/atomos-keyd.sock"),
    }
}

fn load_signing_key(path: &Path) -> Result<Arc<dyn SigningKey>, String> {
    let file = File::open(path).map_err(|e| format!("config: open key: {e}"))?;
    let mut reader = BufReader::new(file);
    let pk = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| format!("config: tls_key: {e}"))?
        .ok_or_else(|| "config: tls_key has no private key".to_string())?;
    any_supported_type(&pk).map_err(|e| format!("config: {e}"))
}

fn sign(key: &dyn SigningKey, message: &[u8]) -> Result<Vec<u8>, String> {
    let signer = key
        .choose_scheme(OFFERED)
        .ok_or_else(|| "config: no rustls signature scheme for key".to_string())?;
    signer
        .sign(message)
        .map_err(|e| format!("config: sign failed: {e}"))
}

fn serve(path: &Path, key: &dyn SigningKey) -> Result<(), String> {
    jail::prepare_socket_dir(path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| format!("config: bind sock: {e}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("config: sock 0600: {e}"))?;
    }
    listener
        .set_nonblocking(false)
        .map_err(|e| format!("config: sock: {e}"))?;
    tracing::info!(sock = %path.display(), "atomos-keyd listening");
    loop {
        let (sock, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                tracing::warn!(%e, "keyd accept");
                continue;
            }
        };
        // Stub peer refuse: other-uid is dropped (fail closed). Cross-uid test
        // needs a second EUID; same `jail::peer_euid_ok` as control_std.
        if !jail::peer_euid_ok(sock.as_raw_fd()) {
            continue;
        }
        handle_conn(sock, key);
    }
}

fn handle_conn(mut sock: UnixStream, key: &dyn SigningKey) {
    let _ = sock.set_read_timeout(Some(IO_TIMEOUT));
    let _ = sock.set_write_timeout(Some(IO_TIMEOUT));
    loop {
        if serve_one(&mut sock, key).is_err() {
            break;
        }
    }
}

fn serve_one(sock: &mut UnixStream, key: &dyn SigningKey) -> std::io::Result<()> {
    let mut lenb = [0u8; 4];
    sock.read_exact(&mut lenb)?;
    let n = u32::from_be_bytes(lenb) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "keyd frame",
        ));
    }
    let mut rest = vec![0u8; n];
    sock.read_exact(&mut rest)?;
    let mut frame = Vec::with_capacity(4 + n);
    frame.extend_from_slice(&lenb);
    frame.extend_from_slice(&rest);
    let (kind, payload) = keyproto::decode_req(&frame)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "keyd req"))?;
    if kind != KIND_SIGN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "keyd kind",
        ));
    }
    let sig =
        sign(key, payload).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    sock.write_all(&keyproto::encode_rep(&sig))?;
    sock.flush()?;
    Ok(())
}
