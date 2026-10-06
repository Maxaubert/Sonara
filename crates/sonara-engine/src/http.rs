//! The shared HTTP client setup (features `kokoro`, `external`): one `ureq`
//! agent shape for the Kokoro model download and the external engines.
//!
//! - TLS through rustls with the platform verifier (Windows' certificate
//!   store), no bundled root certificates.
//! - `http_status_as_error(false)`: 4xx and 5xx bodies are read, so callers
//!   can map the provider's error.
//! - `agent` (the model download) follows redirects and uses the system
//!   proxy. `provider_agent` (external engines, spec 6.4) follows none, so
//!   a key header never goes to a host named in a `Location`, and with
//!   `direct` (a loopback profile) skips the proxy, which `ureq` would
//!   otherwise use for `127.0.0.1` too (it ignores Windows' `<local>`
//!   bypass list). A loopback profile also connects IPv4 first and with a
//!   short connect timeout (`LOOPBACK_CONNECT`): on Windows a refused
//!   loopback connect takes about 2 s, which `localhost` (`::1` first) paid
//!   on every new connection and a stopped local server paid per sentence
//!   before the fallback spoke (#274).
use std::time::Duration;

/// The connect timeout of a remote provider.
pub const REMOTE_CONNECT: Duration = Duration::from_secs(5);
/// The connect timeout of a loopback server, shared by its addresses
/// (IPv4 gets two thirds). A local handshake takes well under 1 ms.
pub const LOOPBACK_CONNECT: Duration = Duration::from_millis(500);

/// The connect timeout for a profile: short for a loopback server.
pub fn connect_timeout(loopback: bool) -> Duration {
    if loopback {
        LOOPBACK_CONNECT
    } else {
        REMOTE_CONNECT
    }
}

/// How long each step of a request may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    /// Until the response headers arrived.
    pub recv_response: Duration,
    /// For the whole body.
    pub recv_body: Duration,
}

fn builder(t: Timeouts) -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(t.connect))
        .timeout_recv_response(Some(t.recv_response))
        .timeout_recv_body(Some(t.recv_body))
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::Rustls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(concat!("sonara/", env!("CARGO_PKG_VERSION")))
}

/// An agent with these timeouts and Sonara's user agent: follows redirects,
/// uses the system proxy (the Kokoro model download).
pub fn agent(t: Timeouts) -> ureq::Agent {
    builder(t).build().into()
}

/// The agent of an external engine: never follows a redirect (a 3xx is the
/// provider's answer, mapped as an error) and, with `direct`, never uses a
/// proxy (a loopback server).
pub fn provider_agent(t: Timeouts, direct: bool) -> ureq::Agent {
    let b = builder(t).max_redirects(0);
    if !direct {
        return b.build().into();
    }
    ureq::Agent::with_parts(
        b.proxy(None).build(),
        ureq::unversioned::transport::DefaultConnector::new(),
        Ipv4First,
    )
}

/// The system resolver with IPv4 addresses first: a local server bound
/// to 127.0.0.1 (most are) answers at once for `localhost`, and one bound
/// only to `::1` is still tried next.
#[derive(Debug)]
struct Ipv4First;

impl ureq::unversioned::resolver::Resolver for Ipv4First {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let mut addrs = ureq::unversioned::resolver::DefaultResolver::default()
            .resolve(uri, config, timeout)?;
        ipv4_first(&mut addrs);
        Ok(addrs)
    }
}

fn ipv4_first(addrs: &mut [std::net::SocketAddr]) {
    // Stable: the system's order within each family is kept.
    addrs.sort_by_key(|a| a.is_ipv6());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_comes_first_and_each_family_keeps_its_order() {
        let mut a: Vec<std::net::SocketAddr> =
            ["[::1]:80", "127.0.0.1:80", "[::2]:80", "127.0.0.2:80"]
                .iter()
                .map(|s| s.parse().unwrap())
                .collect();
        ipv4_first(&mut a);
        let got: Vec<String> = a.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            got,
            ["127.0.0.1:80", "127.0.0.2:80", "[::1]:80", "[::2]:80"]
        );
    }

    #[test]
    fn loopback_connects_get_the_short_timeout() {
        assert_eq!(connect_timeout(true), LOOPBACK_CONNECT);
        assert_eq!(connect_timeout(false), REMOTE_CONNECT);
    }
}
