//! Retries a TCP connect while macOS waits for the user to allow Local Network access.
//!
//! An outgoing connection to a local network address needs that privilege. Until the user
//! answers, and after Don't Allow, a BSD connect fails at once with "No route to host".
//! Nothing reports whether the user has answered, so the connect is tried again for a short
//! time. See Apple's [TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy).

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// How long a connection waits for the user to answer macOS's Local Network prompt.
pub const ANSWER_WAIT: Duration = Duration::from_secs(20);

const RETRY_EVERY: Duration = Duration::from_millis(250);

/// macOS refuses a blocked connection at once. A host that is off, or a `.local` name nobody
/// answers, takes seconds to fail, so a slower failure is not the block.
pub const AT_ONCE: Duration = Duration::from_secs(1);

const LOCAL_NETWORK_SETTING: &str = "If macOS asked whether Marlin may find devices on local networks and you chose Don't Allow, turn on Marlin in System Settings > Privacy & Security > Local Network, then try again.";

/// A failed [`connect`].
#[derive(Debug)]
pub struct Error {
    /// The last attempt's error.
    pub io: std::io::Error,
    /// The connection failed the way macOS fails it while it blocks Local Network access, for all of [`ANSWER_WAIT`].
    pub waited: bool,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.waited {
            write!(f, "{}. {LOCAL_NETWORK_SETTING}", self.io)
        } else {
            write!(f, "{}", self.io)
        }
    }
}

impl std::error::Error for Error {}

/// Opens a TCP connection to `host:port`, trying each resolved address in order within one `timeout`.
/// On macOS, a failure that macOS's Local Network block explains is tried again every 250 ms for up to [`ANSWER_WAIT`], so the connection goes through once the user chooses Allow.
pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, Error> {
    retry_while_blocked(
        || connect_once(host, port, timeout),
        ANSWER_WAIT,
        RETRY_EVERY,
    )
}

/// How one attempt failed.
enum Failure {
    /// The way macOS fails a connection while it blocks Local Network access. Off macOS
    /// nothing fails this way: there "No route to host" to a local address is a real failure.
    Blocked(std::io::Error),
    Other(std::io::Error),
}

/// Runs `attempt` until it succeeds. A failure macOS's Local Network block does not explain
/// stops at once. One it does explain is tried again, because the user's answer can arrive
/// while the prompt is still up and nothing can read that answer.
fn retry_while_blocked<T>(
    mut attempt: impl FnMut() -> Result<T, Failure>,
    limit: Duration,
    every: Duration,
) -> Result<T, Error> {
    let started = Instant::now();
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(Failure::Other(io)) => return Err(Error { io, waited: false }),
            Err(Failure::Blocked(io)) if started.elapsed() >= limit => {
                return Err(Error { io, waited: true })
            }
            Err(Failure::Blocked(_)) => std::thread::sleep(every),
        }
    }
}

fn connect_once(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, Failure> {
    let on_macos = cfg!(target_os = "macos");
    let started = Instant::now();
    let addresses = match (host, port).to_socket_addrs() {
        Ok(addresses) => addresses,
        // Resolving a `.local` name needs the privilege too.
        Err(error) if on_macos && is_local_name(host) && started.elapsed() < AT_ONCE => {
            return Err(Failure::Blocked(error))
        }
        Err(error) => return Err(Failure::Other(error)),
    };
    let deadline = Instant::now() + timeout;
    let mut failure = Failure::Other(std::io::Error::new(
        ErrorKind::NotFound,
        "the name resolved to no address",
    ));
    for address in addresses {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let tried = Instant::now();
        match TcpStream::connect_timeout(&address, left) {
            Ok(stream) => return Ok(stream),
            // Allow can let the blocked address through, so its failure is the one kept.
            Err(_) if matches!(failure, Failure::Blocked(_)) => {}
            Err(error)
                if on_macos
                    && error.kind() == ErrorKind::HostUnreachable
                    && tried.elapsed() < AT_ONCE
                    && is_local_network_address(address.ip()) =>
            {
                failure = Failure::Blocked(error)
            }
            Err(error) => failure = Failure::Other(error),
        }
    }
    Err(failure)
}

fn is_local_name(host: &str) -> bool {
    host.strip_suffix('.')
        .unwrap_or(host)
        .rsplit_once('.')
        .is_some_and(|(name, domain)| !name.is_empty() && domain.eq_ignore_ascii_case("local"))
}

/// TN3179's local network address: multicast, broadcast, or on the subnet of an interface that
/// can broadcast (Wi-Fi or Ethernet; not a VPN or loopback).
fn is_local_network_address(ip: IpAddr) -> bool {
    ip.is_multicast() || ip == IpAddr::V4(Ipv4Addr::BROADCAST) || on_broadcast_subnet(ip)
}

/// nix reads the netmasks: on macOS a netmask's `sa_len` drops its trailing zero bytes, and
/// nix pads them back.
#[cfg(unix)]
fn on_broadcast_subnet(ip: IpAddr) -> bool {
    use nix::net::if_::InterfaceFlags;

    let Ok(interfaces) = nix::ifaddrs::getifaddrs() else {
        return false;
    };
    interfaces
        .filter(|interface| {
            interface
                .flags
                .contains(InterfaceFlags::IFF_UP | InterfaceFlags::IFF_BROADCAST)
        })
        .any(|interface| {
            let (Some(address), Some(netmask)) = (interface.address, interface.netmask) else {
                return false;
            };
            match ip {
                IpAddr::V4(ip) => match (address.as_sockaddr_in(), netmask.as_sockaddr_in()) {
                    (Some(address), Some(netmask)) => {
                        let mask = u32::from(netmask.ip());
                        u32::from(ip) & mask == u32::from(address.ip()) & mask
                    }
                    _ => false,
                },
                IpAddr::V6(ip) => match (address.as_sockaddr_in6(), netmask.as_sockaddr_in6()) {
                    (Some(address), Some(netmask)) => {
                        let mask = u128::from(netmask.ip());
                        u128::from(ip) & mask == u128::from(address.ip()) & mask
                    }
                    _ => false,
                },
            }
        })
}

#[cfg(not(unix))]
fn on_broadcast_subnet(_ip: IpAddr) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv6Addr, TcpListener};

    fn blocked() -> Failure {
        Failure::Blocked(std::io::Error::new(ErrorKind::HostUnreachable, "no route"))
    }

    #[test]
    fn a_success_returns_on_the_first_attempt() {
        let mut tries = 0;
        let value = retry_while_blocked(
            || {
                tries += 1;
                Ok::<_, Failure>(7)
            },
            Duration::from_millis(50),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(tries, 1);
    }

    #[test]
    fn an_unexplained_error_returns_at_once() {
        let mut tries = 0;
        let error = retry_while_blocked(
            || {
                tries += 1;
                Err::<(), _>(Failure::Other(std::io::Error::new(
                    ErrorKind::ConnectionRefused,
                    "refused",
                )))
            },
            Duration::from_millis(50),
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(!error.waited);
        assert_eq!(error.to_string(), "refused");
        assert_eq!(tries, 1);
    }

    #[test]
    fn explained_failures_then_a_success_return_the_value() {
        let mut tries = 0;
        let value = retry_while_blocked(
            || {
                tries += 1;
                if tries < 3 {
                    Err(blocked())
                } else {
                    Ok("open")
                }
            },
            Duration::from_secs(2),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(value, "open");
        assert_eq!(tries, 3);
    }

    #[test]
    fn explained_failures_past_the_limit_name_the_setting() {
        let mut tries = 0;
        let error = retry_while_blocked(
            || {
                tries += 1;
                Err::<(), _>(blocked())
            },
            Duration::from_millis(30),
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(error.waited);
        assert!(tries > 1);
        assert_eq!(
            error.to_string(),
            format!("no route. {LOCAL_NETWORK_SETTING}")
        );
    }

    #[test]
    fn connect_reaches_a_loopback_listener_and_does_not_wait_for_a_closed_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        connect("127.0.0.1", port, Duration::from_secs(2)).unwrap();

        drop(listener);
        let error = connect("127.0.0.1", port, Duration::from_secs(2)).unwrap_err();
        assert!(!error.waited);
    }

    #[test]
    fn multicast_and_broadcast_are_local_and_loopback_is_not() {
        assert!(is_local_network_address(IpAddr::V4(Ipv4Addr::new(
            224, 0, 0, 251
        ))));
        assert!(is_local_network_address(IpAddr::V6(Ipv6Addr::new(
            0xff02, 0, 0, 0, 0, 0, 0, 0xfb
        ))));
        assert!(is_local_network_address(IpAddr::V4(Ipv4Addr::BROADCAST)));
        assert!(!is_local_network_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_local_network_address(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_local_network_address(IpAddr::V4(Ipv4Addr::new(
            8, 8, 8, 8
        ))));
    }

    /// This needs the netmask read right, which macOS's short `sa_len` breaks for a plain read.
    #[cfg(unix)]
    #[test]
    fn this_machines_own_lan_address_is_local() {
        use nix::net::if_::InterfaceFlags;

        let own = nix::ifaddrs::getifaddrs()
            .unwrap()
            .filter(|interface| {
                interface
                    .flags
                    .contains(InterfaceFlags::IFF_UP | InterfaceFlags::IFF_BROADCAST)
            })
            .find_map(|interface| {
                let own = interface.address?.as_sockaddr_in()?.ip();
                Some((own, interface.netmask))
            });
        let Some((own, netmask)) = own else {
            eprintln!("skipped: no IPv4 address on a broadcast interface");
            return;
        };
        let mask = u32::from(netmask.unwrap().as_sockaddr_in().unwrap().ip());
        assert_eq!(
            mask.leading_ones() + mask.trailing_zeros(),
            32,
            "the netmask {mask:#010x} is not a prefix"
        );
        assert!(is_local_network_address(IpAddr::V4(own)));
        // A subnet of two or more addresses holds the address one bit away. A netmask read with
        // a stray last byte would not.
        if mask & 1 == 0 {
            let neighbor = Ipv4Addr::from(u32::from(own) ^ 1);
            assert!(is_local_network_address(IpAddr::V4(neighbor)));
        }
    }

    #[test]
    fn a_local_name_ends_with_local() {
        assert!(is_local_name("nas.local"));
        assert!(is_local_name("NAS.LOCAL."));
        assert!(!is_local_name("nas.lan"));
        assert!(!is_local_name("local"));
        assert!(!is_local_name(".local"));
        assert!(!is_local_name("ñas"));
    }
}
