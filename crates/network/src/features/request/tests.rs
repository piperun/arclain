use crate::features::proxy::ProxyConfig;
use crate::features::whitelist::DomainWhitelist;
use crate::AsyncHttpClient;
use crate::{HttpRequest, RequestStatus};
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
