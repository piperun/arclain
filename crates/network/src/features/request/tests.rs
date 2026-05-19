use crate::features::proxy::ProxyConfig;
use crate::features::request::client::parse_content_range_total;
use crate::features::whitelist::DomainWhitelist;
use crate::AsyncHttpClient;
use crate::{HttpRequest, RequestStatus};

#[test]
fn parse_content_range_total_handles_normal_response() {
    assert_eq!(parse_content_range_total("bytes 0-999/1000"), Some(1000));
    assert_eq!(parse_content_range_total("bytes 500-999/1000"), Some(1000));
}

#[test]
fn parse_content_range_total_handles_unknown_total() {
    // RFC 7233 §4.2: `*` means total length is unknown.
    assert_eq!(parse_content_range_total("bytes 0-999/*"), None);
}

#[test]
fn parse_content_range_total_rejects_garbage() {
    assert_eq!(parse_content_range_total("not a range header"), None);
    assert_eq!(parse_content_range_total(""), None);
    assert_eq!(parse_content_range_total("bytes 0-999/abc"), None);
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

    // Configure test plugin to use proxy (when enabled)
    let mut map = std::collections::HashMap::new();
    map.insert("test-plugin".to_string(), true);
    client.update_plugin_proxy_map(map);

    // 1. Verify direct connection works (even if "use proxy" is true, if proxy config is None, it should build a direct client)
    // Actually, client_proxied is built with None, so it behaves as direct.
    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request 1 failed");
    let status = wait_for_request(&client, &id).await;
    match status {
        RequestStatus::Ready(res) => assert_eq!(res.status_code, 200),
        _ => panic!("Direct request failed"),
    }

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

    // 4. Update back to Direct (Disable proxy)
    let direct_config = ProxyConfig {
        enabled: false,
        address: "127.0.0.1:0".to_string(),
        ..Default::default()
    };
    client.update_config(Some(direct_config));

    // 5. Verify direct connection works again
    let id = client
        .request_for_plugin(
            "test-plugin",
            HttpRequest::get(&format!("{}/test", mock_server.uri())),
        )
        .expect("Request 3 failed");
    let status = wait_for_request(&client, &id).await;
    match status {
        RequestStatus::Ready(res) => assert_eq!(res.status_code, 200),
        _ => panic!("Direct request failed after disabling proxy"),
    }
}

/// Regression test for P2 from `docs/AUDIT_2026-05-03.md`.
///
/// Pre-fix, image_fetch and other callers polled `client.status(id)`
/// every 100ms in a 30-second loop. With ~10 carousel images that's
/// 100 wakes/sec doing nothing while waiting for HTTP.
///
/// Post-fix, callers `await_complete(id).await` and the HTTP-spawned
/// task `notify_waiters` on terminal status. This test verifies the
/// notification path: an awaited completion resolves promptly when
/// the wiremock server responds, far below any plausible 100ms poll
/// quantum that the old loop would have introduced.
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

    // Now await_complete should return without ever blocking on the
    // Notify (the early-exit branch under the lock).
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
    assert_eq!(client.pending_total(), 5, "all 5 requests should be tracked");

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
