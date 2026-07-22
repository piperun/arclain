use crate::features::proxy::ProxyConfig;
use crate::features::request::client::{parse_content_range, ContentRange, RequestCompletion};
use crate::features::whitelist::DomainWhitelist;
use crate::{AsyncHttpClient, StreamingDownload};
use crate::{HttpRequest, HttpResponse, RequestStatus};
use std::io::{Read, Write};
use std::net::TcpListener;

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
    let id = client.request(HttpRequest::get(&format!("{}/large-await", mock_server.uri())));
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
