use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use reqwest::{Url, redirect::Policy};
use roomkit_tools_core::Error;

use crate::WebFetchPolicy;

pub(crate) fn parse(value: &str, max_bytes: usize) -> Result<Url, Error> {
    if value.len() > max_bytes {
        return Err(Error::LimitExceeded("URL is too large".into()));
    }
    let mut url =
        Url::parse(value).map_err(|_| Error::InvalidInput("invalid absolute URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || url.port_or_known_default() == Some(0)
    {
        return Err(Error::InvalidInput(
            "URL requires HTTP or HTTPS and a valid host/port".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Refused("URL credentials are not supported".into()));
    }
    url.set_fragment(None);
    Ok(url)
}

pub(crate) fn normalize(policy: WebFetchPolicy, max_bytes: usize) -> Result<WebFetchPolicy, Error> {
    fn origins(values: Vec<String>, max: usize) -> Result<Vec<String>, Error> {
        values
            .into_iter()
            .map(|value| {
                let url = parse(&value, max)?;
                if url.path() != "/" || url.query().is_some() || value.contains('#') {
                    return Err(Error::InvalidInput(
                        "policy entries must be origins without paths, queries or fragments".into(),
                    ));
                }
                Ok(url.origin().ascii_serialization())
            })
            .collect()
    }
    Ok(WebFetchPolicy {
        allowed_origins: policy
            .allowed_origins
            .map(|v| origins(v, max_bytes))
            .transpose()?,
        private_origins: origins(policy.private_origins, max_bytes)?,
    })
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(matches!(a, 0 | 10 | 127 | 224..=255)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
        || (a == 198 && ((18..=19).contains(&b) || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

// Conservative public-unicast scope; exceptions must be explicit host origins.
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let [a, b, ..] = ip.segments();
            (0x2000..=0x3fff).contains(&a)
                && !(a == 0x2001 && (b < 0x200 || b == 0xdb8))
                && a != 0x2002
                && !(a == 0x3fff && b < 0x1000)
        }
    }
}

pub(crate) fn check_addresses(addresses: &[SocketAddr], private: bool) -> Result<(), Error> {
    if addresses.is_empty() {
        return Err(io(
            "resolve URL host",
            std::io::Error::other("no addresses"),
        ));
    }
    if !private && addresses.iter().any(|address| !public(address.ip())) {
        return Err(Error::Refused(
            "destination resolves outside the permitted public network".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn client(url: &Url, policy: &WebFetchPolicy) -> Result<reqwest::Client, Error> {
    let origin = url.origin().ascii_serialization();
    if policy
        .allowed_origins
        .as_ref()
        .is_some_and(|allowed| !allowed.contains(&origin))
    {
        return Err(Error::Refused("destination origin is not allowed".into()));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| Error::InvalidInput("missing URL port".into()))?;
    let host = url
        .host_str()
        .ok_or_else(|| Error::InvalidInput("missing URL host".into()))?;
    let addresses = match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        _ => tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| io("resolve URL host", e))?
            .collect(),
    };
    check_addresses(&addresses, policy.private_origins.contains(&origin))?;
    // New client per hop: pin validated addresses, disable inherited proxy routes,
    // redirects, cookies and referers. TLS still checks the original hostname.
    reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .referer(false)
        .resolve_to_addrs(host, &addresses)
        .user_agent("roomkit-tools/0.1")
        .build()
        .map_err(|e| io("create HTTP client", e))
}

pub(crate) fn io(
    operation: &'static str,
    source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> Error {
    Error::Io {
        operation,
        source: std::io::Error::other(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_public_addresses_and_mixed_dns_answers_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.2.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public(ip.parse().unwrap()));
        }
        assert!(
            check_addresses(
                &[
                    "8.8.8.8:80".parse().unwrap(),
                    "127.0.0.1:80".parse().unwrap()
                ],
                false
            )
            .is_err()
        );
    }
}
