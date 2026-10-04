//! The shared HTTP client setup (features `kokoro`, `external`): one `ureq`
//! agent shape for the Kokoro model download and the external engines.
//!
//! - TLS through rustls with the platform verifier (Windows' certificate
//!   store), no bundled root certificates; the system proxy applies.
//! - `http_status_as_error(false)`: 4xx and 5xx bodies are read, so callers
//!   can map the provider's error.
use std::time::Duration;

/// How long each step of a request may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    /// Until the response headers arrived.
    pub recv_response: Duration,
    /// For the whole body.
    pub recv_body: Duration,
}

/// An agent with these timeouts and Sonara's user agent.
pub fn agent(t: Timeouts) -> ureq::Agent {
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
        .build()
        .into()
}
