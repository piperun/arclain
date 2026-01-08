use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use arclain_db::{ConfigDb, DbPaths, SecretsDb, SecretsKey, UserConfig};

/// Simple DLSite HTML fetcher that preserves bytes (no UTF-8 decoding).
/// Provides two modes: basic reqwest and arclain_http proxy-aware client.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Product ID, e.g. VJ012345 or RJ999003
    product_id: String,
    /// Output file path
    #[arg(long, default_value = "./scripts/dlsite-fetch/out.html")]
    output: PathBuf,
    /// Base domain (pro|maniax|home). Default pro.
    #[arg(long, default_value = "pro")]
    domain: String,
    /// Socks5 proxy URL override (socks5h://host:port or with auth)
    #[arg(long)]
    socks5: Option<String>,
    /// Optional timeout seconds
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
    /// Use the project arclain_http client (honors configured proxy/headers)
    #[arg(long, default_value_t = false)]
    use_arclain: bool,
}

fn build_url(domain: &str, product_id: &str) -> String {
    format!(
        "https://www.dlsite.com/{}/work/=/product_id/{}.html",
        domain, product_id
    )
}

fn fetch_basic(
    url: &str,
    socks5: Option<&str>,
    timeout: Duration,
) -> Result<(Vec<u8>, Option<String>)> {
    let mut builder = reqwest::blocking::ClientBuilder::new()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(timeout)
        .brotli(true)
        .gzip(true)
        .deflate(true);

    if let Some(proxy) = socks5 {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }

    let client = builder.build()?;
    let resp = client.get(url).send().context("request failed")?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let ct = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = resp.bytes().context("read body")?.to_vec();
    println!(
        "[basic] status={} bytes={} ct={:?}",
        status,
        bytes.len(),
        ct
    );
    Ok((bytes, ct))
}

fn fetch_arclain(
    url: &str,
    timeout: Duration,
    socks5: Option<String>,
) -> Result<(Vec<u8>, Option<String>)> {
    use arclain_http::features::proxy::ProxyConfig;
    use arclain_http::{AsyncHttpClient, DomainWhitelist, HttpMethod, HttpRequest};
    use parking_lot::RwLock;
    use std::sync::Arc;
    use tokio::runtime::Builder;

    // Build a minimal runtime
    let rt = Builder::new_current_thread().enable_all().build()?;

    // Use empty whitelist for this diagnostic tool (host requests bypass whitelist anyway)
    let whitelist = Arc::new(RwLock::new(DomainWhitelist::new()));

    // Build proxy config from socks5 URL if provided
    // Note: select_socks5() may return a full URL like "socks5h://host:port"
    // but ProxyConfig expects just the address portion, so strip the prefix
    let proxy_config = socks5
        .clone()
        .or_else(|| std::env::var("ARCLAIN_SOCKS5").ok())
        .map(|url| {
            // Strip socks5:// or socks5h:// prefix if present
            let address = url
                .strip_prefix("socks5h://")
                .or_else(|| url.strip_prefix("socks5://"))
                .unwrap_or(&url)
                .to_string();
            ProxyConfig {
                enabled: true,
                address,
                username: None,
                password: None,
            }
        });

    // Create client with the runtime handle
    let client = AsyncHttpClient::new(rt.handle().clone(), whitelist, proxy_config.clone());


    let _req = HttpRequest {
        method: HttpMethod::Get,
        url: url.to_string(),
        headers: Default::default(),
        body: None,
        timeout,
    };

    // IMPORTANT: We must use rt.block_on() directly, not client.blocking_get()
    // because blocking_get uses self.runtime (a Handle) which doesn't work
    // properly in a single-threaded runtime context
    let use_proxy = proxy_config.is_some();
    let url_string = url.to_string();
    
    // Extract proxy address for use in async block
    let proxy_address = proxy_config.as_ref().map(|pc| pc.address.clone());
    
    let bytes = rt.block_on(async {
        // Build the reqwest client
        let reqwest_client = if use_proxy {
            // Build proxy URL manually using socks5h:// for remote DNS resolution
            let mut builder = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
            
            if let Some(addr) = &proxy_address {
                // Use socks5h:// for remote DNS resolution (required for Mullvad and similar proxies)
                let proxy_url = format!("socks5h://{}", addr);
                if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
                    builder = builder.proxy(proxy);
                }
            }
            builder.build().map_err(|e| format!("Failed to build client: {}", e))?
        } else {
            reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .build()
                .map_err(|e| format!("Failed to build client: {}", e))?
        };

        let mut req = reqwest_client.get(&url_string);

        // Domain specific headers - mimic Firefox exactly
        if url_string.contains("dlsite.com") {
            println!("[arclain] Injecting DLSite headers");
            req = req.header("Cookie", "adultchecked=1; locale=ja-JP");
            req = req.header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            );
            req = req.header("Accept-Language", "ja-JP,ja;q=0.9,en-US;q=0.8,en;q=0.7");
            req = req.header("Accept-Encoding", "gzip, deflate, br");
            req = req.header("Connection", "keep-alive");
            req = req.header("Upgrade-Insecure-Requests", "1");
            req = req.header("Sec-Fetch-Dest", "document");
            req = req.header("Sec-Fetch-Mode", "navigate");
            req = req.header("Sec-Fetch-Site", "none");
            req = req.header("Sec-Fetch-User", "?1");
            req = req.header("Cache-Control", "no-cache");
            req = req.header("Pragma", "no-cache");
        }

        let response = req
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Failed to read body: {}", e))
    }).map_err(|e| anyhow::anyhow!(e))?;

    // Content-type not returned in blocking_get; best-effort sniffing is omitted to keep raw bytes.
    Ok((bytes, None))
}

fn load_socks5_from_db() -> Result<Option<String>> {
    let paths = DbPaths::calculate_defaults("arclain")?;

    // Load config DB and user_config row
    let cfg_db = ConfigDb::open(&paths.config_db)?;
    let cfg_conn = cfg_db.into_sqlite_db();
    let user_cfg = cfg_conn
        .with_connection(|conn| {
            UserConfig::ensure_table(conn)?;
            Ok(UserConfig::load(conn)?)
        })?
        .unwrap_or_default();

    if !user_cfg.socks5_enabled {
        return Ok(None);
    }

    let address = match user_cfg.socks5_address.as_deref() {
        Some(a) if !a.trim().is_empty() => a.trim().to_string(),
        _ => return Ok(None),
    };

    let mut url = format!("socks5h://{}", address);

    if let Some(username) = user_cfg.socks5_username.as_deref() {
        if let Some(key_path) = paths.key_file.clone() {
            if key_path.exists() {
                if let Ok(key) = SecretsKey::load_from_file(&key_path) {
                    if let Ok(secrets) = SecretsDb::open(&paths.secrets_db, &key.as_bytes()) {
                        if let Ok(Some(pwd)) = secrets.get_secret("proxy:socks5") {
                            let pwd_str: &str = pwd.as_ref();
                            url = format!("socks5h://{}:{}@{}", username, pwd_str, address);
                        } else {
                            url = format!("socks5h://{}@{}", username, address);
                        }
                    }
                }
            }
        }
    }

    Ok(Some(url))
}

fn select_socks5(cli: &Args) -> Option<String> {
    // Precedence: CLI --socks5 > env ARCLAIN_SOCKS5 > DB config
    if let Some(cli_val) = &cli.socks5 {
        return Some(cli_val.clone());
    }
    if let Ok(env_val) = std::env::var("ARCLAIN_SOCKS5") {
        if !env_val.trim().is_empty() {
            return Some(env_val);
        }
    }
    load_socks5_from_db().ok().flatten()
}

fn main() -> Result<()> {
    let args = Args::parse();
    let url = build_url(&args.domain, &args.product_id);
    let timeout = Duration::from_secs(args.timeout_secs);

    let socks5 = select_socks5(&args);

    let (bytes, ct) = if args.use_arclain {
        println!("[mode] arclain_http");
        fetch_arclain(&url, timeout, socks5.clone())?
    } else {
        println!("[mode] basic reqwest");
        fetch_basic(&url, socks5.as_deref(), timeout)?
    };

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, &bytes)?;

    println!("[fetch] url={} bytes={} ct={:?}", url, bytes.len(), ct);
    println!("[fetch] saved to {}", args.output.display());
    Ok(())
}
