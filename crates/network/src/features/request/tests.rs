use crate::features::proxy::ProxyConfig;
use crate::features::rate_limiting::RateLimiter;
use crate::features::request::client::{parse_content_range, ContentRange, RequestCompletion};
use crate::features::request::plugin_policy::{
    validate_plugin_url, validate_redirect_target, validate_resolved_addresses,
    AuthorizedPluginTarget, MAX_PLUGIN_REDIRECTS,
};
use crate::features::request::PluginNetworkPolicy;
use crate::features::whitelist::DomainWhitelist;
use crate::{AsyncHttpClient, StreamingDownload};
use crate::{HttpError, HttpRequest, HttpResponse, RequestStatus};
use std::io::{Read, Write};
use std::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener as TokioTcpListener;

#[test]
fn plugin_policy_rejects_non_http_credentials_fragments_and_missing_hosts() {
    for url in [
        "ftp://example.com/file",
        "file:///etc/passwd",
        "https://user@example.com/",
        "https://user:password@example.com/",
        "https://example.com/page#fragment",
        "https://",
    ] {
        assert!(
            validate_plugin_url(url).is_err(),
            "plugin URL validation accepted {url:?}"
        );
    }

    assert!(validate_plugin_url("https://example.com/path?q=1").is_ok());
}

#[test]
fn plugin_policy_rejects_private_documentation_and_special_addresses() {
    for address in [
        "0.0.0.0",
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.0.0.1",
        "192.0.2.1",
        "192.31.196.1",
        "192.52.193.1",
        "192.88.99.1",
        "192.168.1.1",
        "192.175.48.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "::ffff:8.8.8.8",
        "64:ff9b::1",
        "64:ff9b:1::1",
        "100::1",
        "2001::1",
        "2001:2::1",
        "2001:db8::1",
        "2001:20::1",
        "2002::1",
        "2620:4f:8000::1",
        "fc00::1",
        "fe80::1",
        "fec0::1",
        "ff00::1",
    ] {
        let address = address.parse().expect("valid test IP address");
        assert!(
            validate_resolved_addresses(&[address]).is_err(),
            "plugin address validation accepted {address}"
        );
    }

    for address in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
        let address = address.parse().expect("valid public test IP address");
        assert!(
            validate_resolved_addresses(&[address]).is_ok(),
            "plugin address validation rejected public address {address}"
        );
    }
}

#[test]
fn plugin_policy_rejects_an_entire_mixed_dns_answer() {
    let answers = [
        "93.184.216.34".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
    ];

    assert!(validate_resolved_addresses(&answers).is_err());
    assert!(validate_resolved_addresses(&[]).is_err());
}

#[test]
fn plugin_policy_zero_rpm_denies_and_limits_are_isolated_per_plugin() {
    let limiter = RateLimiter::new(60);

    assert!(!limiter.try_acquire_with_limit("plugin-a\0example.com", 0));
    assert!(limiter.try_acquire_with_limit("Plugin-A\0Example.COM", 1));
    assert!(!limiter.try_acquire_with_limit("Plugin-A\0example.com", 1));
    assert!(
        limiter.try_acquire_with_limit("plugin-a\0example.com", 1),
        "case-distinct plugin IDs must have isolated budgets"
    );
}

#[test]
fn plugin_policy_redirect_targets_join_relative_and_repeat_syntax_checks() {
    let current = validate_plugin_url("https://example.com/a/start").unwrap();
    let relative = validate_redirect_target(&current, "../next?q=1").unwrap();
    assert_eq!(relative.as_str(), "https://example.com/next?q=1");

    for location in [
        "ftp://example.com/file",
        "https://user@example.com/secret",
        "/next#fragment",
        "https://",
    ] {
        assert!(
            validate_redirect_target(&current, location).is_err(),
            "redirect validation accepted {location:?}"
        );
    }
}

#[tokio::test]
async fn plugin_policy_requires_registered_enabled_network_capability() {
    let client = AsyncHttpClient::new(
        Handle::current(),
        Arc::new(parking_lot::RwLock::new(DomainWhitelist::default())),
        None,
    );
    let request = HttpRequest::get("https://example.com/");

    assert!(matches!(
        client.request_for_plugin("missing", request.clone()),
        Err(HttpError::PluginNetworkNotConfigured { .. })
    ));

    client.configure_plugin(
        "disabled",
        PluginNetworkPolicy {
            network_enabled: false,
            requests_per_minute: 60,
        },
    );
    assert!(matches!(
        client.request_for_plugin("disabled", request),
        Err(HttpError::PluginNetworkDisabled { .. })
    ));
}

#[tokio::test]
async fn plugin_policy_rejects_authority_and_hop_by_hop_headers_before_connecting() {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind sentinel HTTP listener");
    let address = listener.local_addr().expect("sentinel listener address");
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "headers.test");
    let client = AsyncHttpClient::new(Handle::current(), whitelist, None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("headers.test", vec![vec![address]]);
    let url = format!("http://headers.test:{}/", address.port());

    for header in [
        "Host",
        "Proxy-Authorization",
        "Content-Length",
        "Connection",
        "Proxy-Connection",
        "Keep-Alive",
        "Transfer-Encoding",
        "Upgrade",
        "TE",
        "Trailer",
        "Forwarded",
        "X-Forwarded-Host",
    ] {
        let result = client.request_for_plugin(
            "plugin-a",
            HttpRequest::get(&url).with_header(header, "attacker.internal"),
        );
        assert!(
            matches!(result, Err(HttpError::SecurityWarning { .. })),
            "plugin-controlled routing header {header:?} was accepted: {result:?}"
        );
    }

    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "a rejected plugin header still reached the network"
    );
}

#[tokio::test]
async fn plugin_policy_derives_the_wire_host_from_the_validated_url() {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind HTTP listener");
    let address = listener.local_addr().expect("HTTP listener address");
    let (host_sender, host_receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept HTTP request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut buffer).await.expect("read HTTP request");
            assert!(read > 0, "connection closed before request headers");
            request.extend_from_slice(&buffer[..read]);
        }
        let request = String::from_utf8(request).expect("HTTP request headers are UTF-8");
        let host = request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("host")
                    .then(|| value.trim().to_string())
            })
            .expect("wire request has Host header");
        host_sender.send(host).expect("receive observed Host");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .expect("write HTTP response");
    });

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "authority.test");
    let client = AsyncHttpClient::new(Handle::current(), whitelist, None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("authority.test", vec![vec![address]]);
    let url = format!("http://authority.test:{}/", address.port());

    let id = client
        .request_for_plugin("plugin-a", HttpRequest::get(url))
        .expect("checked request should start");
    assert!(
        matches!(
            wait_for_request(&client, &id).await,
            RequestStatus::Ready(_)
        ),
        "ordinary checked request failed"
    );
    assert_eq!(
        host_receiver.await.expect("observe wire Host"),
        format!("authority.test:{}", address.port())
    );
    server.await.expect("HTTP server task panicked");
}

#[tokio::test]
async fn plugin_policy_socks_connector_sends_the_pinned_ip_not_the_hostname() {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind SOCKS5 listener");
    let proxy_address = listener.local_addr().expect("SOCKS5 listener address");
    let (address_sender, address_receiver) = tokio::sync::oneshot::channel();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept SOCKS5 client");
        let mut greeting = [0_u8; 3];
        socket
            .read_exact(&mut greeting)
            .await
            .expect("read SOCKS5 greeting");
        assert_eq!(greeting, [0x05, 0x01, 0x00]);
        socket
            .write_all(&[0x05, 0x00])
            .await
            .expect("select SOCKS5 no-auth");

        let mut request = [0_u8; 4];
        socket
            .read_exact(&mut request)
            .await
            .expect("read SOCKS5 CONNECT prefix");
        assert_eq!(&request[..3], &[0x05, 0x01, 0x00]);
        let atyp = request[3];
        let destination = match atyp {
            0x01 => {
                let mut octets = [0_u8; 4];
                socket.read_exact(&mut octets).await.unwrap();
                std::net::IpAddr::V4(octets.into())
            }
            0x04 => {
                let mut octets = [0_u8; 16];
                socket.read_exact(&mut octets).await.unwrap();
                std::net::IpAddr::V6(octets.into())
            }
            0x03 => {
                let length = socket.read_u8().await.unwrap() as usize;
                let mut hostname = vec![0_u8; length];
                socket.read_exact(&mut hostname).await.unwrap();
                panic!(
                    "SOCKS5 connector leaked hostname {:?} instead of using pinned DNS",
                    String::from_utf8_lossy(&hostname)
                );
            }
            other => panic!("unexpected SOCKS5 address type {other:#x}"),
        };
        let mut port = [0_u8; 2];
        socket.read_exact(&mut port).await.unwrap();
        let _ = address_sender.send((atyp, destination, u16::from_be_bytes(port)));

        // The HTTP result is irrelevant; acknowledge CONNECT then close.
        socket
            .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
    });

    let client = AsyncHttpClient::new(
        Handle::current(),
        Arc::new(parking_lot::RwLock::new(DomainWhitelist::default())),
        Some(ProxyConfig {
            enabled: true,
            address: proxy_address.to_string(),
            username: None,
            password: None,
        }),
    );
    let pinned_ip = "93.184.216.34".parse().unwrap();
    let target = AuthorizedPluginTarget {
        url: url::Url::parse("http://pinned.example/resource").unwrap(),
        use_proxy: true,
        resolved: vec![std::net::SocketAddr::new(pinned_ip, 80)],
    };
    let pinned_client = client
        .build_pinned_plugin_client(&target)
        .expect("build pinned SOCKS5 plugin client");

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        pinned_client.get(target.url.clone()).send(),
    )
    .await;
    let (atyp, destination, port) = address_receiver
        .await
        .expect("SOCKS5 listener did not observe CONNECT");
    assert_eq!(atyp, 0x01);
    assert_eq!(destination, pinned_ip);
    assert_eq!(port, 80);
    server.await.expect("SOCKS5 listener task panicked");
}

#[tokio::test]
async fn plugin_policy_proxy_selection_fails_closed_without_an_enabled_proxy() {
    async fn assert_fails_closed(proxy_config: Option<ProxyConfig>) {
        let listener = TokioTcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind direct-connection sentinel");
        let address = listener.local_addr().expect("sentinel address");
        let (accepted_sender, accepted_receiver) = tokio::sync::oneshot::channel();
        let sentinel = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept direct request");
            let _ = accepted_sender.send(());
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.expect("read direct request");
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ndirect",
                )
                .await
                .expect("write sentinel response");
        });

        let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
        whitelist.write().approve("plugin-a", "proxy-required.test");
        let client = AsyncHttpClient::new(Handle::current(), whitelist, proxy_config);
        configure_enabled_plugin(&client, "plugin-a", 60);
        client.allow_special_plugin_addresses_for_test();
        client.set_plugin_dns_answers_for_test("proxy-required.test", vec![vec![address]]);
        client.update_plugin_proxy_map(std::collections::HashMap::from([(
            "plugin-a".to_string(),
            true,
        )]));
        let url = format!("http://proxy-required.test:{}/", address.port());

        let id = client
            .request_for_plugin("plugin-a", HttpRequest::get(url))
            .expect("checked request should start");
        let status = wait_for_request(&client, &id).await;
        assert!(
            matches!(status, RequestStatus::Failed(ref message) if message.contains("enabled proxy")),
            "proxy-selected request did not fail closed: {status:?}"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), accepted_receiver)
                .await
                .is_err(),
            "proxy-selected request fell back to a direct connection"
        );
        sentinel.abort();
    }

    assert_fails_closed(None).await;
    assert_fails_closed(Some(ProxyConfig {
        enabled: false,
        address: "127.0.0.1:9".to_string(),
        username: None,
        password: None,
    }))
    .await;
}

fn configure_enabled_plugin(client: &AsyncHttpClient, plugin_id: &str, rpm: u32) {
    client.configure_plugin(
        plugin_id,
        PluginNetworkPolicy {
            network_enabled: true,
            requests_per_minute: rpm,
        },
    );
}

#[tokio::test]
async fn plugin_policy_rejects_mixed_dns_answers_even_when_whitelisted() {
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "example.com");
    let client = AsyncHttpClient::new(Handle::current(), whitelist.clone(), None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.set_plugin_dns_answers_for_test(
        "example.com",
        vec![vec![
            "93.184.216.34:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ]],
    );

    let id = client
        .request_for_plugin("plugin-a", HttpRequest::get("https://example.com/"))
        .expect("checked request should start asynchronously");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Failed(ref message) if message.contains("unsafe address")),
        "mixed DNS answer was not rejected: {status:?}"
    );
    assert_eq!(client.plugin_dns_lookup_count_for_test("example.com"), 1);
}

#[tokio::test]
async fn plugin_policy_pins_the_single_authorized_dns_resolution() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pinned"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pinned"))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "pinned.test");
    let client = AsyncHttpClient::new(Handle::current(), whitelist.clone(), None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test(
        "pinned.test",
        vec![
            vec![*server.address()],
            vec!["127.0.0.2:9".parse().unwrap()],
        ],
    );

    let url = format!("http://pinned.test:{}/pinned", server.address().port());
    let id = client
        .request_for_plugin("plugin-a", HttpRequest::get(url))
        .expect("checked request should start");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Ready(ref response) if response.status_code == 200),
        "pinned request failed: {status:?}"
    );
    assert_eq!(
        client.plugin_dns_lookup_count_for_test("pinned.test"),
        1,
        "the HTTP connector performed a second plugin-target DNS resolution"
    );
}

#[tokio::test]
async fn plugin_policy_reauthorizes_redirects_and_strips_cross_origin_secrets() {
    let server = MockServer::start().await;
    let final_url = format!("http://final.test:{}/final", server.address().port());
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(302).append_header("Location", final_url.as_str()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "start.test");
    whitelist.write().approve("plugin-a", "final.test");
    let client = AsyncHttpClient::new(Handle::current(), whitelist, None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("start.test", vec![vec![*server.address()]]);
    client.set_plugin_dns_answers_for_test("final.test", vec![vec![*server.address()]]);

    let start_url = format!("http://start.test:{}/start", server.address().port());
    let request = HttpRequest::get(start_url)
        .with_header("Authorization", "Bearer plugin-secret")
        .with_header("Cookie", "session=plugin-secret")
        .with_header("X-Plugin-Trace", "kept");
    let id = client
        .request_for_plugin("plugin-a", request)
        .expect("checked redirect request should start");
    let status = wait_for_request(&client, &id).await;
    assert!(matches!(status, RequestStatus::Ready(_)), "{status:?}");

    let requests = server.received_requests().await.unwrap();
    let start = requests
        .iter()
        .find(|request| request.url.path() == "/start")
        .expect("initial request was not received");
    let final_request = requests
        .iter()
        .find(|request| request.url.path() == "/final")
        .expect("redirect request was not received");
    assert!(start.headers.contains_key("authorization"));
    assert!(start.headers.contains_key("cookie"));
    assert!(!final_request.headers.contains_key("authorization"));
    assert!(!final_request.headers.contains_key("cookie"));
    assert!(final_request.headers.contains_key("x-plugin-trace"));
    assert_eq!(client.plugin_dns_lookup_count_for_test("start.test"), 1);
    assert_eq!(client.plugin_dns_lookup_count_for_test("final.test"), 1);
}

#[tokio::test]
async fn plugin_policy_rejects_an_unapproved_redirect_after_resolving_it() {
    let server = MockServer::start().await;
    let blocked_url = format!("http://blocked.test:{}/blocked", server.address().port());
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(302).append_header("Location", blocked_url.as_str()))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "start.test");
    let client = AsyncHttpClient::new(Handle::current(), whitelist.clone(), None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("start.test", vec![vec![*server.address()]]);
    client.set_plugin_dns_answers_for_test("blocked.test", vec![vec![*server.address()]]);

    let start_url = format!("http://start.test:{}/start", server.address().port());
    let id = client
        .request_for_plugin("plugin-a", HttpRequest::get(start_url))
        .expect("checked redirect request should start");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Failed(ref message) if message.contains("not whitelisted")),
        "unapproved redirect was not rejected: {status:?}"
    );
    assert_eq!(client.plugin_dns_lookup_count_for_test("blocked.test"), 1);
    assert!(whitelist
        .read()
        .get_pending()
        .iter()
        .any(|entry| entry.plugin_id == "plugin-a" && entry.domain == "blocked.test"));
}

#[tokio::test]
async fn plugin_policy_follows_five_redirects_and_rejects_the_sixth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/loop"))
        .respond_with(ResponseTemplate::new(302).append_header("Location", "/loop"))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "loop.test");
    let client = AsyncHttpClient::new(Handle::current(), whitelist, None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test(
        "loop.test",
        vec![vec![*server.address()]; MAX_PLUGIN_REDIRECTS + 1],
    );

    let url = format!("http://loop.test:{}/loop", server.address().port());
    let id = client
        .request_for_plugin("plugin-a", HttpRequest::get(url))
        .expect("checked redirect request should start");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Failed(ref message) if message.contains("redirect limit")),
        "redirect cap was not enforced: {status:?}"
    );
    assert_eq!(
        client.plugin_dns_lookup_count_for_test("loop.test"),
        MAX_PLUGIN_REDIRECTS + 1
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        MAX_PLUGIN_REDIRECTS + 1
    );
}

#[tokio::test]
async fn plugin_policy_checked_blocking_get_uses_the_pinned_executor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/buffered"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"checked"))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "buffered.test");
    let client = Arc::new(AsyncHttpClient::new(Handle::current(), whitelist, None));
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("buffered.test", vec![vec![*server.address()]]);
    let url = format!("http://buffered.test:{}/buffered", server.address().port());
    let worker_client = client.clone();

    let body = tokio::task::spawn_blocking(move || {
        worker_client.blocking_get_for_plugin("plugin-a", &url)
    })
    .await
    .expect("checked buffered worker panicked")
    .expect("checked buffered request failed");

    assert_eq!(body, b"checked");
    assert_eq!(client.plugin_dns_lookup_count_for_test("buffered.test"), 1);
}

#[tokio::test]
async fn plugin_policy_checked_streaming_get_uses_the_pinned_executor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"streamed"))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "stream.test");
    let client = Arc::new(AsyncHttpClient::new(Handle::current(), whitelist, None));
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("stream.test", vec![vec![*server.address()]]);
    let url = format!("http://stream.test:{}/stream", server.address().port());
    let worker_client = client.clone();

    let (download, bytes) = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        let download = worker_client
            .blocking_get_streaming_for_plugin("plugin-a", &url, &mut bytes, None, None);
        (download, bytes)
    })
    .await
    .expect("checked streaming worker panicked");
    let download = download.expect("checked streaming request failed");

    assert_eq!(bytes, b"streamed");
    assert_eq!(download.bytes_written, 8);
    assert!(!download.was_partial);
    assert_eq!(client.plugin_dns_lookup_count_for_test("stream.test"), 1);
}

#[tokio::test]
async fn plugin_policy_checked_streaming_preserves_range_and_if_match_headers() {
    let listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind resumable HTTP listener");
    let address = listener.local_addr().expect("resumable listener address");
    let (headers_sender, headers_receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept resumable request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket
                .read(&mut buffer)
                .await
                .expect("read resumable request");
            assert!(read > 0, "connection closed before resumable headers");
            request.extend_from_slice(&buffer[..read]);
        }
        let request = String::from_utf8(request).expect("HTTP headers are UTF-8");
        let range = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("range:"))
            .map(str::to_string);
        let if_match = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("if-match:"))
            .map(str::to_string);
        headers_sender
            .send((range, if_match))
            .expect("receive resumable headers");
        socket
            .write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef",
            )
            .await
            .expect("write resumable response");
    });

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "resume.test");
    let client = Arc::new(AsyncHttpClient::new(Handle::current(), whitelist, None));
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("resume.test", vec![vec![address]]);
    let url = format!("http://resume.test:{}/file", address.port());
    let worker_client = client.clone();
    let (download, bytes) = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        let download = worker_client.blocking_get_streaming_for_plugin(
            "plugin-a",
            &url,
            &mut bytes,
            Some(3),
            Some("\"etag-v1\""),
        );
        (download, bytes)
    })
    .await
    .expect("checked resumable worker panicked");

    let download = download.expect("checked resumable request failed");
    assert_eq!(bytes, b"def");
    assert!(download.was_partial);
    assert_eq!(download.total_size, Some(6));
    let (range, if_match) = headers_receiver.await.expect("observe resumable headers");
    assert_eq!(range.as_deref(), Some("range: bytes=3-"));
    assert_eq!(if_match.as_deref(), Some("if-match: \"etag-v1\""));
    server.await.expect("resumable HTTP server panicked");
}

#[tokio::test]
async fn plugin_policy_preserves_dlsite_cdn_path_normalization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/images/RJ361000/RJ361000_img.jpg"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("plugin-a", "dlsite.jp");
    let client = AsyncHttpClient::new(Handle::current(), whitelist, None);
    configure_enabled_plugin(&client, "plugin-a", 60);
    client.allow_special_plugin_addresses_for_test();
    client.set_plugin_dns_answers_for_test("img.dlsite.jp", vec![vec![*server.address()]]);
    let url = format!(
        "http://img.dlsite.jp:{}/images/RJ00361000/RJ361000_img.jpg",
        server.address().port()
    );

    let id = client
        .request_for_plugin("plugin-a", HttpRequest::get(url))
        .expect("checked DLSite request should start");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Ready(ref response) if response.status_code == 200),
        "DLSite CDN path normalization was lost: {status:?}"
    );
}

#[test]
fn parse_content_range_accepts_exact_known_ranges() {
    assert_eq!(
        parse_content_range("bytes 0-999/1000"),
        Some(ContentRange {
            start: 0,
            end: 999,
            total: 1000,
        })
    );
    assert_eq!(
        parse_content_range("bytes 500-999/1000"),
        Some(ContentRange {
            start: 500,
            end: 999,
            total: 1000,
        })
    );
}

#[test]
fn parse_content_range_rejects_non_exact_or_impossible_ranges() {
    for header in [
        "not a range header",
        "",
        "bytes 0-999/abc",
        "bytes 0-999/*",
        "items 0-999/1000",
        "Bytes 0-999/1000",
        "bytes 0-999 /1000",
        " bytes 0-999/1000",
        "bytes 0-999/1000 ",
        "bytes 999-0/1000",
        "bytes 0-1000/1000",
        "bytes 0-0/0",
        "bytes +0-9/10",
    ] {
        assert_eq!(
            parse_content_range(header),
            None,
            "unexpectedly accepted {header:?}"
        );
    }
}
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn wait_for_request(client: &AsyncHttpClient, id: &crate::RequestId) -> RequestStatus {
    loop {
        if let Some(status) = client.status(id) {
            match status {
                RequestStatus::Ready(_) | RequestStatus::Failed(_) | RequestStatus::Cancelled => {
                    return status;
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn raw_http_response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n").into_bytes();
    for (name, value) in headers {
        response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    response
}

fn spawn_raw_http_server(response: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    let address = listener.local_addr().expect("loopback server address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept streaming request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set request read timeout");

        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read streaming request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }

        stream.write_all(&response).expect("write raw response");
        stream.flush().expect("flush raw response");
    });
    (format!("http://{address}/range"), server)
}

async fn execute_raw_streaming_response(
    response: Vec<u8>,
    start_byte: Option<u64>,
) -> (Result<StreamingDownload, String>, Vec<u8>) {
    let (url, server) = spawn_raw_http_server(response);
    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);

    let result = tokio::task::spawn_blocking(move || {
        let mut buffer = Vec::new();
        let result = client.blocking_get_streaming(&url, false, &mut buffer, start_byte, None);
        (result, buffer)
    })
    .await
    .expect("streaming worker did not panic");

    server.join().expect("loopback server did not panic");
    result
}

#[tokio::test]
async fn test_direct_connection() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/test"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None); // No proxy

    let id = client.request(HttpRequest::get(&format!("{}/test", mock_server.uri())));
    let status = wait_for_request(&client, &id).await;

    match status {
        RequestStatus::Ready(response) => {
            assert_eq!(response.status_code, 200);
        }
        _ => panic!("Request failed: {:?}", status),
    }
}

#[tokio::test]
async fn test_proxy_application_failure() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    // Whitelist the domain for the plugin check
    whitelist.write().approve("test-plugin", "127.0.0.1"); // Mock server runs on local loopback

    // Configure an invalid proxy address
    let proxy_config = ProxyConfig {
        enabled: true,
        address: "127.0.0.1:0".to_string(), // Invalid port
        username: None,
        password: None,
    };

    let client = AsyncHttpClient::new(handle, whitelist, Some(proxy_config));
    client.configure_plugin(
        "test-plugin",
        PluginNetworkPolicy {
            network_enabled: true,
            requests_per_minute: 60,
        },
    );
    client.allow_special_plugin_addresses_for_test();

    // Enable proxy for test plugin
    let mut map = std::collections::HashMap::new();
    map.insert("test-plugin".to_string(), true);
    client.update_plugin_proxy_map(map);

    // Use request_for_plugin to trigger proxy usage
    // Note: This relies on whitelist check passing. MockServer usually binds to 127.0.0.1.
    // request_for_plugin performs security/whitelist checks.
    // We need to make sure 127.0.0.1 is allowed or use request() if we could invoke proxied logic directly.
    // But request() forces false.
    // So we must satisfy request_for_plugin checks.

    // Whitelist is already setup above.

    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request start failed");

    let status = wait_for_request(&client, &id).await;

    match status {
        RequestStatus::Failed(_) => {
            // Success - we expected it to fail due to bad proxy
        }
        RequestStatus::Ready(res) => {
            // It might succeed if it somehow bypassed proxy or proxy was ignored.
            panic!(
                "Request succeeded: status {}, but should have failed due to invalid proxy",
                res.status_code
            );
        }
        _ => panic!("Unexpected status: {:?}", status),
    }
}

#[tokio::test]
async fn test_runtime_config_update() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/test"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    whitelist.write().approve("test-plugin", "127.0.0.1");

    // Start Direct
    let client = AsyncHttpClient::new(handle.clone(), whitelist.clone(), None);
    client.configure_plugin(
        "test-plugin",
        PluginNetworkPolicy {
            network_enabled: true,
            requests_per_minute: 60,
        },
    );
    client.allow_special_plugin_addresses_for_test();

    // Configure test plugin to use proxy (when enabled)
    let mut map = std::collections::HashMap::new();
    map.insert("test-plugin".to_string(), true);
    client.update_plugin_proxy_map(map);

    // 1. A plugin explicitly routed through a proxy must fail closed when no
    // proxy is configured.
    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request 1 failed");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Failed(ref message) if message.contains("enabled proxy")),
        "proxy-selected request without a proxy did not fail closed: {status:?}"
    );

    // 2. Update to Invalid Proxy
    let proxy_config = ProxyConfig {
        enabled: true,
        address: "127.0.0.1:0".to_string(),
        username: None,
        password: None,
    };
    client.update_config(Some(proxy_config));

    // 3. Verify request now fails (because "test-plugin" is mapped to true)
    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request 2 failed");
    let status = wait_for_request(&client, &id).await;
    match status {
        RequestStatus::Failed(_) => {}
        _ => panic!("Request should have failed with invalid proxy"),
    }

    // 4. A disabled proxy also fails closed while plugin routing still asks
    // for a proxy.
    let direct_config = ProxyConfig {
        enabled: false,
        address: "127.0.0.1:0".to_string(),
        ..Default::default()
    };
    client.update_config(Some(direct_config));

    // 5. Verify a disabled proxy does not silently become a direct request.
    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request 3 failed");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Failed(ref message) if message.contains("enabled proxy")),
        "disabled proxy did not fail closed: {status:?}"
    );

    // 6. Explicitly route the plugin directly, then host-only direct behavior
    // remains available.
    client.update_plugin_proxy_map(std::collections::HashMap::from([(
        "test-plugin".to_string(),
        false,
    )]));
    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request 4 failed");
    let status = wait_for_request(&client, &id).await;
    assert!(
        matches!(status, RequestStatus::Ready(ref response) if response.status_code == 200),
        "explicit direct routing failed: {status:?}"
    );
}

/// Regression test for P2 from `docs/AUDIT_2026-05-03.md`.
///
/// Pre-fix, image_fetch and other callers polled `client.status(id)`
/// every 100ms in a 30-second loop. With ~10 carousel images that's
/// 100 wakes/sec doing nothing while waiting for HTTP.
///
/// Post-fix, callers `await_complete(id).await` subscribe to a watch
/// cell carrying the latest status. This test verifies the completion
/// path: an awaited completion resolves promptly when the wiremock
/// server responds, far below any plausible 100ms poll quantum that
/// the old loop would have introduced.
#[tokio::test]
async fn p2_await_complete_resolves_on_completion() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/quick"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);

    let id = client.request(HttpRequest::get(&format!("{}/quick", mock_server.uri())));

    let start = std::time::Instant::now();
    let status = client
        .await_complete(&id)
        .await
        .expect("await_complete returned None for known id");
    let elapsed = start.elapsed();

    match status {
        RequestStatus::Ready(res) => assert_eq!(res.status_code, 200),
        other => panic!("Expected Ready, got {:?}", other),
    }

    // Sanity: this is much faster than a 100ms poll-tick path (which
    // was the previous shape).
    assert!(
        elapsed < Duration::from_millis(500),
        "await_complete took {:?} — looks like polling, not notify",
        elapsed,
    );
}

/// `await_complete` returns immediately if the request has already
/// reached a terminal state.
#[tokio::test]
async fn p2_await_complete_returns_immediately_when_already_done() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/done"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);

    let id = client.request(HttpRequest::get(&format!("{}/done", mock_server.uri())));
    // Drain via the legacy poll helper so we know status is terminal.
    let _ = wait_for_request(&client, &id).await;

    // Now await_complete should observe the terminal value stored in
    // the completion cell without blocking for another transition.
    let start = std::time::Instant::now();
    let status = client.await_complete(&id).await;
    assert!(matches!(status, Some(RequestStatus::Ready(_))));
    assert!(start.elapsed() < Duration::from_millis(50));
}

/// `await_complete` for an unknown id returns None.
#[tokio::test]
async fn p2_await_complete_returns_none_for_unknown_id() {
    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);

    let bogus = crate::RequestId::new();
    let result = client.await_complete(&bogus).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn terminal_state_sent_before_wait_is_still_observed() {
    let completion = RequestCompletion::new(RequestStatus::Pending);
    completion.set(RequestStatus::Failed("finished-before-wait".into()));

    tokio::time::timeout(Duration::from_millis(100), completion.wait())
        .await
        .expect("stored completion must not wait for another edge");

    let status = completion.status();
    assert!(
        matches!(status, RequestStatus::Failed(ref message) if message == "finished-before-wait")
    );
}

#[tokio::test]
async fn completion_during_waiter_setup_is_not_lost() {
    let completion = RequestCompletion::new(RequestStatus::Pending);
    let waiter = completion.wait();

    // The terminal update lands after the caller has created its wait
    // future but before that future gets its first poll.
    completion.set(RequestStatus::Failed("during-setup".into()));

    tokio::time::timeout(Duration::from_millis(100), waiter)
        .await
        .expect("completion during waiter setup must be stored");
    let status = completion.status();
    assert!(matches!(status, RequestStatus::Failed(ref message) if message == "during-setup"));
}

#[test]
fn clone_counter_detects_clones_of_the_watched_completion_state() {
    let completion = RequestCompletion::new(RequestStatus::Ready(HttpResponse {
        status_code: 200,
        headers: Default::default(),
        body: vec![0x3c; 1024 * 1024],
        content_type: None,
    }));
    assert_eq!(completion.status_clone_count(), 0);

    completion.clone_watched_state_for_test();

    assert_eq!(
        completion.status_clone_count(),
        1,
        "the counter must catch deep clones of the watch payload"
    );
}

#[tokio::test]
async fn internal_completion_checks_do_not_clone_ready_response_bodies() {
    let mock_server = MockServer::start().await;
    let body = vec![0x5a; 1024 * 1024];
    Mock::given(method("GET"))
        .and(path("/large-completion"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);
    let id = client.request(HttpRequest::get(&format!(
        "{}/large-completion",
        mock_server.uri()
    )));
    let completion = client
        .completion_for_test(&id)
        .expect("new request has a completion cell");

    tokio::time::timeout(Duration::from_secs(2), completion.wait())
        .await
        .expect("request should complete");
    assert_eq!(
        completion.status_clone_count(),
        0,
        "waiting for a terminal state must not clone the response body"
    );

    assert_eq!(client.pending_count(), 0);
    assert_eq!(
        completion.status_clone_count(),
        0,
        "pending_count must inspect status by reference"
    );

    let taken = client
        .take_response(&id)
        .expect("terminal response remains available");
    assert!(matches!(
        taken,
        RequestStatus::Ready(ref response) if response.body.len() == body.len()
    ));
    assert_eq!(
        completion.status_clone_count(),
        1,
        "take_response should clone only the owned value returned publicly"
    );
}

#[tokio::test]
async fn await_complete_clones_ready_response_only_for_its_return_value() {
    let mock_server = MockServer::start().await;
    let body = vec![0xa5; 1024 * 1024];
    Mock::given(method("GET"))
        .and(path("/large-await"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);
    let id = client.request(HttpRequest::get(&format!(
        "{}/large-await",
        mock_server.uri()
    )));
    let completion = client
        .completion_for_test(&id)
        .expect("new request has a completion cell");

    let status = client
        .await_complete(&id)
        .await
        .expect("request remains publicly tracked");
    assert!(matches!(
        status,
        RequestStatus::Ready(ref response) if response.body.len() == body.len()
    ));
    assert_eq!(
        completion.status_clone_count(),
        1,
        "await_complete should clone only its owned return value"
    );
}

#[tokio::test]
async fn cancellation_wakes_an_existing_waiter_before_removal() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cancel"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);
    let id = client.request(HttpRequest::get(&format!("{}/cancel", mock_server.uri())));

    let waiter = client.await_complete(&id);
    tokio::pin!(waiter);
    tokio::select! {
        biased;
        result = &mut waiter => panic!("request completed before cancellation: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }

    client.cancel(&id);

    let result = tokio::time::timeout(Duration::from_millis(100), &mut waiter)
        .await
        .expect("cancellation must wake an already-subscribed waiter");
    assert!(
        result.is_none(),
        "cancelled entries remain removed publicly"
    );
}

#[test]
fn late_worker_updates_do_not_overwrite_terminal_states() {
    let failed = RequestCompletion::new(RequestStatus::Failed("first failure".into()));
    failed.set(RequestStatus::InProgress);
    assert!(matches!(
        failed.status(),
        RequestStatus::Failed(ref message) if message == "first failure"
    ));

    let cancelled = RequestCompletion::new(RequestStatus::Cancelled);
    cancelled.set(RequestStatus::Ready(HttpResponse {
        status_code: 200,
        headers: Default::default(),
        body: b"late response".to_vec(),
        content_type: None,
    }));
    assert!(matches!(cancelled.status(), RequestStatus::Cancelled));

    let ready = RequestCompletion::new(RequestStatus::Ready(HttpResponse {
        status_code: 204,
        headers: Default::default(),
        body: Vec::new(),
        content_type: None,
    }));
    ready.set(RequestStatus::Failed("late failure".into()));
    assert!(matches!(
        ready.status(),
        RequestStatus::Ready(ref response) if response.status_code == 204
    ));
}

/// Regression test for P19 from `docs/AUDIT_2026-05-03.md`.
///
/// Pre-fix, `AsyncHttpClient::cancel(id)` flipped the entry's status
/// to `Cancelled` and left the entry inside the `pending` map. Long
/// sessions that fired-and-forgot many requests (carousel scroll,
/// abandoned image fetches) would accumulate cancelled entries
/// indefinitely — each holding the full `HttpResponse` body for any
/// requests that completed before the cancel call.
///
/// Post-fix, `cancel(id)` removes the entry outright (still notifying
/// any awaiters first). `pending_total()` reflects the actual map
/// size and is what this test asserts on.
#[tokio::test]
async fn p19_cancel_removes_entry_from_pending_map() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/leak"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);

    // Fire 5 requests; their entries land in `pending` synchronously
    // inside `request()` before the spawned HTTP task runs.
    let ids: Vec<_> = (0..5)
        .map(|_| client.request(HttpRequest::get(&format!("{}/leak", mock_server.uri()))))
        .collect();
    assert_eq!(
        client.pending_total(),
        5,
        "all 5 requests should be tracked"
    );

    // Cancel each. Pre-fix this would have left 5 Cancelled entries
    // sitting in the map; post-fix the map empties.
    for id in &ids {
        client.cancel(id);
    }
    assert_eq!(
        client.pending_total(),
        0,
        "P19 regression: cancel() left entries in the pending map",
    );

    // Cancelling again is a no-op (entry already gone).
    for id in &ids {
        client.cancel(id);
    }
    assert_eq!(client.pending_total(), 0);

    // Subsequent reads on a cancelled id return None.
    assert!(client.status(&ids[0]).is_none());
    assert!(client.take_response(&ids[0]).is_none());
}

/// Streaming GET writes the body chunk-by-chunk into the caller's
/// writer and never holds the full body in memory. The
/// `StreamingDownload` result reports the bytes-written count and
/// captures ETag / Last-Modified for resume validation.
#[tokio::test]
async fn streaming_get_writes_body_to_writer() {
    let mock_server = MockServer::start().await;
    let body = vec![0xab_u8; 12_345]; // arbitrary mid-size buffer
    Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"abc-v1\"")
                .insert_header("last-modified", "Wed, 21 Oct 2026 07:28:00 GMT")
                .set_body_bytes(body.clone()),
        )
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);
    let url = format!("{}/blob", mock_server.uri());

    // Run the blocking streaming call from a spawn_blocking task — the
    // outer #[tokio::test] runtime is multi-threaded so `block_on`
    // inside the client is safe here.
    let result = tokio::task::spawn_blocking(move || {
        let mut buf: Vec<u8> = Vec::new();
        let result = client.blocking_get_streaming(&url, false, &mut buf, None, None);
        (result, buf)
    })
    .await
    .unwrap();

    let (info, buf) = result;
    let info = info.expect("streaming download succeeded");
    assert_eq!(info.bytes_written, body.len() as u64);
    assert_eq!(buf.len(), body.len());
    assert!(!info.was_partial, "200 response → was_partial = false");
    assert_eq!(info.etag.as_deref(), Some("\"abc-v1\""));
    assert_eq!(
        info.last_modified.as_deref(),
        Some("Wed, 21 Oct 2026 07:28:00 GMT")
    );
    assert_eq!(info.total_size, Some(body.len() as u64));
}

/// Range request with start_byte returns 206 Partial Content; the
/// `was_partial` flag flips so the caller knows to append (not
/// truncate) the prior partial bytes.
#[tokio::test]
async fn streaming_range_request_reports_partial() {
    let mock_server = MockServer::start().await;
    let body = vec![0x42_u8; 1000];
    let tail = body[500..].to_vec();
    Mock::given(method("GET"))
        .and(path("/range"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", format!("bytes 500-999/{}", body.len()))
                .insert_header("etag", "\"resume-tag\"")
                .set_body_bytes(tail.clone()),
        )
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);
    let url = format!("{}/range", mock_server.uri());

    let result = tokio::task::spawn_blocking(move || {
        let mut buf: Vec<u8> = Vec::new();
        let info = client
            .blocking_get_streaming(&url, false, &mut buf, Some(500), Some("\"resume-tag\""))
            .expect("range request succeeded");
        (info, buf)
    })
    .await
    .unwrap();

    let (info, buf) = result;
    assert!(info.was_partial, "206 response → was_partial = true");
    assert_eq!(info.bytes_written, 500);
    assert_eq!(buf.len(), 500);
    // total_size comes from Content-Range, not Content-Length, on 206.
    assert_eq!(info.total_size, Some(1000));
}

#[tokio::test]
async fn streaming_range_request_preserves_full_response_fallback() {
    let mock_server = MockServer::start().await;
    let body = b"complete replacement body".to_vec();
    Mock::given(method("GET"))
        .and(path("/range-fallback"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&mock_server)
        .await;

    let handle = Handle::current();
    let whitelist = Arc::new(parking_lot::RwLock::new(DomainWhitelist::default()));
    let client = AsyncHttpClient::new(handle, whitelist, None);
    let url = format!("{}/range-fallback", mock_server.uri());

    let (info, buffer) = tokio::task::spawn_blocking(move || {
        let mut buffer = Vec::new();
        let info = client
            .blocking_get_streaming(&url, false, &mut buffer, Some(5), None)
            .expect("200 range fallback succeeded");
        (info, buffer)
    })
    .await
    .unwrap();

    assert!(!info.was_partial, "200 fallback must replace partial data");
    assert_eq!(info.bytes_written, body.len() as u64);
    assert_eq!(info.total_size, Some(body.len() as u64));
    assert_eq!(buffer, body);
}

#[tokio::test]
async fn partial_response_rejects_invalid_headers_before_writing() {
    for (label, content_range, start_byte) in [
        ("wrong start", "bytes 4-8/10", Some(5)),
        ("invalid unit", "items 5-9/10", Some(5)),
        ("end before start", "bytes 5-4/10", Some(5)),
        ("total does not exceed end", "bytes 5-9/9", Some(5)),
    ] {
        let response = raw_http_response(
            "206 Partial Content",
            &[("Content-Range", content_range), ("Content-Length", "5")],
            b"abcde",
        );
        let (result, buffer) = execute_raw_streaming_response(response, start_byte).await;

        assert!(result.is_err(), "{label} header unexpectedly succeeded");
        assert!(
            buffer.is_empty(),
            "{label} header wrote bytes before validation"
        );
    }
}

#[tokio::test]
async fn partial_response_requires_content_range_before_writing() {
    let response = raw_http_response("206 Partial Content", &[("Content-Length", "5")], b"abcde");
    let (result, buffer) = execute_raw_streaming_response(response, Some(5)).await;

    assert!(result.is_err());
    assert!(buffer.is_empty(), "missing Content-Range wrote body bytes");
}

#[tokio::test]
async fn partial_response_requires_a_requested_range_before_writing() {
    let response = raw_http_response(
        "206 Partial Content",
        &[("Content-Range", "bytes 0-4/10"), ("Content-Length", "5")],
        b"abcde",
    );
    let (result, buffer) = execute_raw_streaming_response(response, None).await;

    assert!(result.is_err());
    assert!(buffer.is_empty(), "unexpected 206 wrote body bytes");
}

#[tokio::test]
async fn partial_response_rejects_mismatched_content_length_before_writing() {
    let response = raw_http_response(
        "206 Partial Content",
        &[("Content-Range", "bytes 5-9/10"), ("Content-Length", "4")],
        b"abcd",
    );
    let (result, buffer) = execute_raw_streaming_response(response, Some(5)).await;

    assert!(result.is_err());
    assert!(
        buffer.is_empty(),
        "mismatched Content-Length wrote body bytes"
    );
}

#[tokio::test]
async fn partial_response_rejects_a_truncated_body() {
    let response = raw_http_response(
        "206 Partial Content",
        &[
            ("Content-Range", "bytes 5-9/10"),
            ("Transfer-Encoding", "chunked"),
        ],
        b"3\r\nabc\r\n0\r\n\r\n",
    );
    let (result, buffer) = execute_raw_streaming_response(response, Some(5)).await;

    assert!(result.is_err(), "truncated range unexpectedly succeeded");
    assert_eq!(buffer, b"abc", "streamed bytes should remain observable");
}

#[tokio::test]
async fn partial_response_never_writes_past_the_declared_range() {
    let response = raw_http_response(
        "206 Partial Content",
        &[
            ("Content-Range", "bytes 5-9/10"),
            ("Transfer-Encoding", "chunked"),
        ],
        b"7\r\nabcdefg\r\n0\r\n\r\n",
    );
    let (result, buffer) = execute_raw_streaming_response(response, Some(5)).await;

    let error = result.expect_err("overlong range unexpectedly succeeded");
    assert_eq!(buffer.len(), 5, "writer crossed the declared byte count");
    assert_eq!(
        buffer, b"abcde",
        "bytes beyond the declared range reached the writer"
    );
    assert!(
        error.contains("exceeds Content-Range"),
        "unclear overlong-range error: {error}"
    );
}
