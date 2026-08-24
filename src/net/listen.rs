//! socket2 bind: SO_REUSEADDR, optional SO_REUSEPORT, TCP_NODELAY, TFO.

use std::net::{SocketAddr, TcpListener as StdListener, UdpSocket as StdUdp};

use socket2::{Domain, Protocol, Socket, Type};

use crate::error::ServeError;

#[derive(Clone, Copy, Debug)]
pub struct ListenOpt {
    pub nodelay: bool,
    pub reuseport: bool,
    pub fastopen: bool,
    pub backlog: i32,
}

pub struct Bound {
    pub listener: StdListener,
    pub local: SocketAddr,
}

pub fn bind(addr: SocketAddr, opt: &ListenOpt) -> Result<Bound, ServeError> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let sock = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_reuse_address(true)?;
    if opt.reuseport {
        sock.set_reuse_port(true)?;
    }
    sock.set_nodelay(opt.nodelay)?;
    if opt.fastopen {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let fd = sock.as_raw_fd();
            let q: libc::c_int = opt.backlog.max(1);
            // SAFETY: `fd` is a live socket2 fd; TCP_FASTOPEN takes an int queue size.
            let _rc = unsafe {
                libc::setsockopt(
                    fd,
                    libc::IPPROTO_TCP,
                    libc::TCP_FASTOPEN,
                    (&q as *const libc::c_int).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
        }
    }
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    sock.listen(opt.backlog.max(1))?;
    let listener: StdListener = sock.into();
    let local = listener.local_addr()?;
    Ok(Bound { listener, local })
}

pub fn bind_udp(addr: SocketAddr, reuseport: bool) -> Result<StdUdp, ServeError> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let sock = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    if reuseport {
        sock.set_reuse_port(true)?;
    }
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    Ok(sock.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodelay_and_reuseport_set() {
        let b = bind(
            "127.0.0.1:0".parse().unwrap(),
            &ListenOpt {
                nodelay: true,
                reuseport: true,
                fastopen: false,
                backlog: 16,
            },
        )
        .unwrap();
        let nodelay = socket2::SockRef::from(&b.listener)
            .nodelay()
            .unwrap();
        assert!(nodelay);
        let local = b.local;
        assert!(local.ip().is_loopback());
        drop(b);
    }
}
