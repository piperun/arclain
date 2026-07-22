//! Security policy for plugin-originated HTTP requests.

use crate::shared::HttpError;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use url::Url;

/// Maximum number of redirects followed for one checked plugin request.
pub(crate) const MAX_PLUGIN_REDIRECTS: usize = 5;

/// Network capability and request budget registered for one plugin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginNetworkPolicy {
    pub network_enabled: bool,
    pub requests_per_minute: u32,
}

/// A plugin target whose URL and resolved addresses passed policy checks.
pub(crate) struct AuthorizedPluginTarget {
    pub(crate) url: Url,
    pub(crate) proxy_config: Option<crate::features::proxy::ProxyConfig>,
    pub(crate) resolved: Vec<SocketAddr>,
}

/// Parse the subset of URLs plugins are permitted to request.
pub(crate) fn validate_plugin_url(value: &str) -> Result<Url, HttpError> {
    let url = Url::parse(value).map_err(|error| HttpError::InvalidUrl {
        reason: error.to_string(),
    })?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(HttpError::InvalidUrl {
            reason: "plugin URLs must use HTTP or HTTPS".to_string(),
        });
    }
    if url.host_str().is_none() {
        return Err(HttpError::InvalidUrl {
            reason: "plugin URL has no host".to_string(),
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpError::InvalidUrl {
            reason: "plugin URLs must not contain credentials".to_string(),
        });
    }
    if url.fragment().is_some() {
        return Err(HttpError::InvalidUrl {
            reason: "plugin URLs must not contain fragments".to_string(),
        });
    }

    Ok(url)
}

/// Resolve a redirect Location and repeat the plugin URL syntax policy before
/// the target reaches DNS resolution.
pub(crate) fn validate_redirect_target(current: &Url, location: &str) -> Result<Url, HttpError> {
    let target = current
        .join(location)
        .map_err(|error| HttpError::InvalidUrl {
            reason: format!("invalid redirect Location: {error}"),
        })?;
    validate_plugin_url(target.as_str())
}

/// Reject plugin-controlled headers that can change HTTP routing, proxy
/// credentials, framing, or hop-by-hop connection semantics. The host derives
/// these values from the validated URL and request body instead.
pub(crate) fn validate_plugin_headers(headers: &HashMap<String, String>) -> Result<(), HttpError> {
    const FORBIDDEN: &[&str] = &[
        "host",
        "proxy-authorization",
        "content-length",
        "connection",
        "proxy-connection",
        "keep-alive",
        "transfer-encoding",
        "upgrade",
        "te",
        "trailer",
        "forwarded",
    ];

    if let Some(name) = headers.keys().find(|name| {
        FORBIDDEN
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
            || name
                .get(.."x-forwarded-".len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("x-forwarded-"))
    }) {
        return Err(HttpError::SecurityWarning {
            message: format!("plugin requests must not set routing header {name:?}"),
        });
    }

    Ok(())
}

/// Reject DNS results unless every answer is an ordinary public address.
pub(crate) fn validate_resolved_addresses(addresses: &[IpAddr]) -> Result<(), HttpError> {
    if addresses.is_empty() {
        return Err(HttpError::DnsResolutionFailed {
            host: "<unknown>".to_string(),
            reason: "resolver returned no addresses".to_string(),
        });
    }

    if let Some(address) = addresses
        .iter()
        .copied()
        .find(|address| !is_public_address(*address))
    {
        return Err(HttpError::UnsafeResolvedAddress { address });
    }

    Ok(())
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => {
            if address.to_ipv4_mapped().is_some() {
                return false;
            }
            is_public_ipv6(address)
        }
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _fourth] = address.octets();

    match first {
        0 | 10 | 127 | 224..=255 => false,
        100 if (64..=127).contains(&second) => false,
        169 if second == 254 => false,
        172 if (16..=31).contains(&second) => false,
        192 if second == 0 && third == 0 => false,
        192 if second == 0 && third == 2 => false,
        192 if second == 31 && third == 196 => false,
        192 if second == 52 && third == 193 => false,
        192 if second == 88 && third == 99 => false,
        192 if second == 168 => false,
        192 if second == 175 && third == 48 => false,
        198 if matches!(second, 18 | 19) => false,
        198 if second == 51 && third == 100 => false,
        203 if second == 0 && third == 113 => false,
        _ => true,
    }
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let segments = address.segments();

    // Globally-routed unicast currently occupies 2000::/3. Exclude the
    // special-purpose and documentation allocations inside that range too.
    if !(0x2000..=0x3fff).contains(&segments[0]) {
        return false;
    }
    if segments[0] == 0x2001 && segments[1] <= 0x01ff {
        return false; // IETF protocol assignments, benchmarking, ORCHIDv1.
    }
    if segments[0] == 0x2001 && (0x0020..=0x002f).contains(&segments[1]) {
        return false; // ORCHIDv2.
    }
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return false; // Documentation.
    }
    if segments[0] == 0x2002 {
        return false; // Deprecated 6to4 transition space.
    }
    if segments[0] == 0x3fff && segments[1] <= 0x0fff {
        return false; // Documentation prefix 3fff::/20.
    }
    if segments[..3] == [0x2620, 0x004f, 0x8000] {
        return false; // AS112-v6 special-purpose anycast.
    }

    true
}
