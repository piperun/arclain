/// Proxy configuration
#[derive(Clone, Debug, Default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub address: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    pub fn to_proxy(&self) -> Option<reqwest::Proxy> {
        if !self.enabled || self.address.is_empty() {
            return None;
        }

        // Use socks5h:// (with 'h') to perform DNS resolution through the proxy
        // This is required for hostnames that are only resolvable via the proxy
        let url = match (&self.username, &self.password) {
            (Some(u), Some(p)) => format!("socks5h://{}:{}@{}", u, p, self.address),
            _ => format!("socks5h://{}", self.address),
        };

        reqwest::Proxy::all(&url).ok()
    }

    /// Test the connection with current configuration
    /// Returns a success message with IP and Country or an error
    pub async fn test_connection(&self) -> anyhow::Result<String> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(10));

        if let Some(proxy) = self.to_proxy() {
            builder = builder.proxy(proxy);
        } else if self.enabled {
            return Err(anyhow::anyhow!("Invalid proxy configuration"));
        }

        let client = builder
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build client: {}", e))?;

        // Test connection using ip-api.com for IP and Location data
        let response = client
            .get("http://ip-api.com/json")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Connection failed: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("HTTP error: {}", response.status()));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON: {}", e))?;

        let ip = json
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown IP");
        let country = json
            .get("country")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Country");

        Ok(format!("Connected via {} ({})", ip, country))
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
