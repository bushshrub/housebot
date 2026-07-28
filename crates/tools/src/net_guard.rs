//! SSRF guard: a DNS resolver that refuses to hand non-public addresses to the
//! HTTP client.
//!
//! Validating a URL and then letting the HTTP client resolve the name a second
//! time leaves a DNS-rebinding window: a short-TTL record can answer with a
//! public address for the check and a loopback address for the connect.
//! [`PublicOnlyResolver`] closes that window by making the blocklist part of the
//! resolution the client actually connects with.
//!
//! Address classification itself lives in `housebot_sandbox::validation` so the
//! sandbox's clone-URL check and the agent's fetch tools cannot drift apart.

use std::net::SocketAddr;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

pub use housebot_sandbox::validation::is_blocked_ip;

/// Resolver that fails closed when a name resolves to any non-public address.
///
/// Failing on *any* blocked address rather than filtering them out keeps the
/// behaviour identical to the up-front URL check, so a host that mixes public
/// and private answers is rejected instead of silently half-allowed.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?
                .collect();
            if let Some(blocked) = resolved.iter().find(|address| is_blocked_ip(address.ip())) {
                return Err(
                    format!("{host} resolves to non-public address {}", blocked.ip()).into(),
                );
            }
            if resolved.is_empty() {
                return Err(format!("{host} did not resolve to any address").into());
            }
            Ok(Box::new(resolved.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolver_rejects_names_that_resolve_to_loopback() {
        let resolved = PublicOnlyResolver
            .resolve("localhost".parse().expect("name should parse"))
            .await;
        match resolved {
            Ok(_) => panic!("localhost must not resolve through the guard"),
            Err(error) => assert!(
                error.to_string().contains("non-public address"),
                "unexpected error: {error}"
            ),
        }
    }

    #[tokio::test]
    async fn resolver_reports_lookup_failures() {
        let resolved = PublicOnlyResolver
            .resolve("invalid.invalid".parse().expect("name should parse"))
            .await;
        assert!(
            resolved.is_err(),
            "a name that does not resolve should be an error"
        );
    }
}
