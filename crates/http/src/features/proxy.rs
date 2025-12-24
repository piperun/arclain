/// Proxy configuration
#[derive(Clone, Debug, Default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub address: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Convert configuration to a reqwest Proxy
    pub fn to_proxy(&self) -> Option<reqwest::Proxy> {
        if !self.enabled || self.address.is_empty() {
            return None;
        }

        let url = match (&self.username, &self.password) {
            (Some(u), Some(p)) => format!("socks5://{}:{}@{}", u, p, self.address),
            _ => format!("socks5://{}", self.address),
        };

        reqwest::Proxy::all(&url).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_disabled() {
        let config = ProxyConfig {
            enabled: false,
            address: "127.0.0.1:9050".to_string(),
            ..Default::default()
        };
        assert!(config.to_proxy().is_none());
    }

    #[test]
    fn test_proxy_empty_address() {
        let config = ProxyConfig {
            enabled: true,
            address: "".to_string(),
            ..Default::default()
        };
        assert!(config.to_proxy().is_none());
    }

    #[test]
    fn test_proxy_enabled_no_auth() {
        let config = ProxyConfig {
            enabled: true,
            address: "127.0.0.1:9050".to_string(),
            ..Default::default()
        };
        assert!(config.to_proxy().is_some());
    }

    #[test]
    fn test_proxy_enabled_with_auth() {
        let config = ProxyConfig {
            enabled: true,
            address: "127.0.0.1:9050".to_string(),
            username: Some("user".to_string()),
            password: Some("pass".to_string()),
        };
        assert!(config.to_proxy().is_some());
    }
}
