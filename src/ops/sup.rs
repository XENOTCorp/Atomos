//! Supervisor: one child worker per core, restart on death. Not on the datapath.
//!
//! nginx-style isolation: a worker `panic=abort` does not kill siblings.
//! This process only forks/execs; it does not accept HTTP.

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::ServeError;

static SUP_STOP: AtomicBool = AtomicBool::new(false);
static SUP_HUP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_hup(_: libc::c_int) {
    SUP_HUP.store(true, Ordering::SeqCst);
}

extern "C" fn on_term(_: libc::c_int) {
    SUP_STOP.store(true, Ordering::SeqCst);
}

pub struct WorkerSpec {
    pub exe: std::path::PathBuf,
    pub args: Vec<String>,
    pub n: u32,
    pub shutdown_timeout: Duration,
}

impl WorkerSpec {
    pub fn with_n(exe: std::path::PathBuf, args: Vec<String>, n: u32) -> Self {
        Self {
            exe,
            args,
            n,
            shutdown_timeout: Duration::from_secs(2),
        }
    }
}

/// Spawn `n` workers (`ATOMOS_WORKER_INDEX` set). Restarts a child that exits.
/// Never returns on success.
pub fn run(spec: WorkerSpec) -> Result<(), ServeError> {
    let n = spec.n.max(1);
    #[cfg(unix)]
    {
        let h = on_term as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t;
        unsafe {
            libc::signal(libc::SIGTERM, h);
            libc::signal(libc::SIGINT, h);
            libc::signal(
                libc::SIGHUP,
                on_hup as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t,
            );
        }
    }
    let mut kids: Vec<Option<Child>> = Vec::with_capacity(n as usize);
    for i in 0..n {
        kids.push(Some(spawn_one(&spec, i)?));
    }
    loop {
        if SUP_STOP.load(Ordering::SeqCst) {
            drain(&mut kids, spec.shutdown_timeout);
            return Ok(());
        }
        if SUP_HUP.swap(false, Ordering::SeqCst) {
            match spawn_generation(&spec, n) {
                Ok(new) => {
                    drain(&mut kids, spec.shutdown_timeout);
                    kids = new;
                }
                Err(e) => tracing::error!(%e, "SIGHUP spawn failed; keeping old generation"),
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        for (i, slot) in kids.iter_mut().enumerate() {
            let dead = slot
                .as_mut()
                .map(|c| c.try_wait().ok().flatten().is_some())
                .unwrap_or(true);
            if dead {
                tracing::warn!(index = i, "worker exit; restart");
                *slot = Some(spawn_one(&spec, i as u32)?);
            }
        }
    }
}

fn spawn_generation(spec: &WorkerSpec, n: u32) -> Result<Vec<Option<Child>>, ServeError> {
    let mut kids = Vec::with_capacity(n as usize);
    for i in 0..n {
        kids.push(Some(spawn_one(spec, i)?));
    }
    Ok(kids)
}

fn drain(kids: &mut [Option<Child>], timeout: Duration) {
    for c in kids.iter_mut().flatten() {
        #[cfg(unix)]
        unsafe {
            libc::kill(c.id() as libc::pid_t, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let mut live = false;
        for slot in kids.iter_mut() {
            if let Some(c) = slot.as_mut() {
                match c.try_wait() {
                    Ok(Some(_)) => *slot = None,
                    _ => live = true,
                }
            }
        }
        if !live {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for slot in kids.iter_mut() {
        if let Some(c) = slot.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
            *slot = None;
        }
    }
}

fn spawn_one(spec: &WorkerSpec, i: u32) -> Result<Child, ServeError> {
    Command::new(&spec.exe)
        .args(&spec.args)
        .env("ATOMOS_WORKER_INDEX", i.to_string())
        .spawn()
        .map_err(ServeError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_n_at_least_one() {
        let s = WorkerSpec::with_n("true".into(), vec![], 0);
        assert_eq!(s.n.max(1), 1);
    }

    #[test]
    fn shutdown_timeout_default_is_2s() {
        let s = WorkerSpec::with_n("true".into(), vec![], 1);
        assert_eq!(s.shutdown_timeout, Duration::from_secs(2));
    }

    #[test]
    fn drain_kills_after_timeout() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let mut kids = vec![Some(child)];
        drain(&mut kids, Duration::from_millis(200));
        assert!(kids.iter().all(|k| k.is_none()));
        let still = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap();
        assert!(!still.success());
    }
}
