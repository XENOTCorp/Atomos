//! Exercise the real post-bind sandbox in a child, never in the test runner.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn sandboxed_epoll_serves_small_and_sendfile_responses() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("static");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("index.html"), b"sandbox-ok").unwrap();
    let big: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
    std::fs::write(root.join("big.bin"), &big).unwrap();
    let rules = dir.path().join("rules.json");
    std::fs::write(
        &rules,
        br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"]}]}"#,
    )
    .unwrap();

    for (seccomp, landlock) in [(true, false), (false, true), (true, true)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let config = dir.path().join("config.json");
        let cfg = serde_json::json!({
            "bind": addr.to_string(), "workers": 1, "cpu_pin": false,
            "static_root": root, "rules_path": rules,
            "control_socket": dir.path().join("ctl.sock"),
            "memory_cap_bytes": 6_000_000_000u64,
            "seccomp": seccomp, "landlock": landlock
        });
        std::fs::write(&config, serde_json::to_vec(&cfg).unwrap()).unwrap();
        let log = tempfile::tempfile().unwrap();
        let mut server = Server(
            Command::new(env!("CARGO_BIN_EXE_atomos"))
                .arg("--config")
                .arg(&config)
                .current_dir(dir.path())
                .env("ATOMOS_HOST", dir.path().join("no-host.json"))
                .env("ATOMOS_SF_MIN", "131072")
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log.try_clone().unwrap()))
                .spawn()
                .unwrap(),
        );
        let result = (|| -> std::io::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if server.0.try_wait()?.is_some() {
                    return Err(std::io::Error::other("server exited before readiness"));
                }
                if TcpStream::connect(addr).is_ok() {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::other("server readiness timeout"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            for (path, expected) in [
                ("/", b"sandbox-ok".as_slice()),
                ("/big.bin", big.as_slice()),
            ] {
                let mut stream = TcpStream::connect(addr)?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                write!(
                    stream,
                    "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                )?;
                let mut response = Vec::new();
                stream.read_to_end(&mut response)?;
                if !response.starts_with(b"HTTP/1.1 200 ") {
                    return Err(std::io::Error::other("expected HTTP 200"));
                }
                let Some(end) = response.windows(4).position(|w| w == b"\r\n\r\n") else {
                    return Err(std::io::Error::other("missing response headers"));
                };
                if &response[end + 4..] != expected {
                    return Err(std::io::Error::other("response contents differ"));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let mut logs = String::new();
            use std::os::unix::fs::FileExt;
            let mut bytes = vec![0; log.metadata().unwrap().len() as usize];
            log.read_at(&mut bytes, 0).unwrap();
            logs.push_str(&String::from_utf8_lossy(&bytes));
            panic!("seccomp={seccomp} landlock={landlock}: {error}\n{logs}");
        }
    }
}
