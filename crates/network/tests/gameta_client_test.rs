use arclain_network::features::gameta_client::{GametaClient, ServerConfig};
use wiremock::matchers::{bearer_token, body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// GametaClient uses reqwest::blocking under the hood. Blocking calls must not
// run on the async executor thread, so each test dispatches the client call
// via `tokio::task::spawn_blocking`.

#[tokio::test]
async fn test_health_check_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(r#"{"status":"ok","version":"0.4.3"}"#, "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).health()
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    let health = result.unwrap();
    assert_eq!(health.status, "ok");
    assert_eq!(health.version, "0.4.3");
}

#[tokio::test]
async fn test_health_check_non_200_is_err() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(503).set_body_raw(
            r#"{"status":"degraded","version":"0.4.3"}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).health()
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_err(), "expected Err on 503, got Ok");
}

#[tokio::test]
async fn test_get_metadata_found() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/RJ123456"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{
                    "id": "dlsite:RJ123456",
                    "source": "dlsite",
                    "title": "Test Game",
                    "creator": "Test Circle",
                    "tags": ["RPG"],
                    "extras": {}
                }"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig {
            url,
            api_key: Some("test-key".to_string()),
        })
        .get_metadata("dlsite", "RJ123456")
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    let meta = result.unwrap().expect("expected Some metadata");
    assert_eq!(meta.title.as_deref(), Some("Test Game"));
    assert_eq!(meta.creator.as_deref(), Some("Test Circle"));
}

#[tokio::test]
async fn test_get_metadata_not_found_returns_none() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/NONEXISTENT"))
        .respond_with(ResponseTemplate::new(404).set_body_raw(
            r#"{"error":"Not found","code":"NOT_FOUND"}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).get_metadata("dlsite", "NONEXISTENT")
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(
        matches!(result, Ok(None)),
        "expected Ok(None) on 404, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_get_metadata_server_error_is_err() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/RJ000001"))
        .respond_with(ResponseTemplate::new(500).set_body_raw(
            r#"{"error":"Internal server error","code":"INTERNAL"}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).get_metadata("dlsite", "RJ000001")
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_err(), "expected Err on 500, got: {:?}", result);
    assert!(
        result.unwrap_err().contains("Internal server error"),
        "error message should include server error text"
    );
}

#[tokio::test]
async fn test_fetch_metadata() {
    let server = MockServer::start().await;

    let expected_body = serde_json::json!({
        "source": "dlsite",
        "id": "RJ999999",
        "force": false
    });

    Mock::given(method("POST"))
        .and(path("/api/v1/fetch"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{
                    "status": "success",
                    "source": "dlsite",
                    "id": "RJ999999",
                    "metadata": {
                        "id": "dlsite:RJ999999",
                        "source": "dlsite",
                        "title": "Fetched Game",
                        "tags": [],
                        "extras": {}
                    }
                }"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig {
            url,
            api_key: Some("key".to_string()),
        })
        .fetch_metadata("dlsite", "RJ999999", false)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    let resp = result.unwrap();
    assert_eq!(resp.status, "success");
    assert_eq!(resp.source, "dlsite");
    assert_eq!(resp.id, "RJ999999");
    let meta = resp.metadata.expect("expected metadata in fetch response");
    assert_eq!(meta.title.as_deref(), Some("Fetched Game"));
}

#[tokio::test]
async fn test_fetch_metadata_force_flag() {
    let server = MockServer::start().await;

    let expected_body = serde_json::json!({
        "source": "dlsite",
        "id": "RJ111111",
        "force": true
    });

    Mock::given(method("POST"))
        .and(path("/api/v1/fetch"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{
                    "status": "refreshed",
                    "source": "dlsite",
                    "id": "RJ111111",
                    "metadata": null
                }"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None })
            .fetch_metadata("dlsite", "RJ111111", true)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    assert_eq!(result.unwrap().status, "refreshed");
}

#[tokio::test]
async fn test_search_with_source_and_limit() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .and(query_param("q", "test"))
        .and(query_param("source", "dlsite"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{
                    "query": "test",
                    "source": "dlsite",
                    "results": [
                        {
                            "id": "RJ123",
                            "source": "dlsite",
                            "title": "Result 1"
                        }
                    ]
                }"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).search(
            "test",
            Some("dlsite"),
            Some(10),
        )
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    let resp = result.unwrap();
    assert_eq!(resp.query, "test");
    assert_eq!(resp.source.as_deref(), Some("dlsite"));
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].title, "Result 1");
    assert_eq!(resp.results[0].id, "RJ123");
}

#[tokio::test]
async fn test_search_without_optional_params() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .and(query_param("q", "hello"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"query":"hello","source":null,"results":[]}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).search("hello", None, None)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    let resp = result.unwrap();
    assert_eq!(resp.results.len(), 0);
}

#[tokio::test]
async fn test_server_unreachable_returns_err() {
    // Port 1 is reserved and never open — connection will be refused immediately.
    let result = tokio::task::spawn_blocking(|| {
        GametaClient::new(ServerConfig {
            url: "http://127.0.0.1:1".to_string(),
            api_key: None,
        })
        .health()
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_err(), "expected Err when server is unreachable");
}

#[tokio::test]
async fn test_auth_header_sent_on_get_metadata() {
    let server = MockServer::start().await;

    // The mock only matches when the correct Bearer token is present.
    // If the header is absent or wrong, wiremock returns 404 (no matched mock),
    // which would cause get_metadata to return Ok(None) — and the .expect(1)
    // assertion at server drop would fail. Both failure modes are caught.
    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/RJ001"))
        .and(bearer_token("my-secret-key"))
        .respond_with(ResponseTemplate::new(404).set_body_raw(
            r#"{"error":"Not found","code":"NOT_FOUND"}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig {
            url,
            api_key: Some("my-secret-key".to_string()),
        })
        .get_metadata("dlsite", "RJ001")
    })
    .await
    .expect("spawn_blocking panicked");

    // 404 with correct auth → Ok(None)
    assert!(
        matches!(result, Ok(None)),
        "expected Ok(None) — auth header must have been present, got: {:?}",
        result
    );
    // .expect(1) on the mock is verified at MockServer drop
}

#[tokio::test]
async fn test_no_auth_header_when_api_key_absent() {
    let server = MockServer::start().await;

    // This mock requires NO Authorization header. If the client incorrectly
    // sends one, wiremock won't match it and .expect(1) will fail at drop.
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(r#"{"status":"ok","version":"1.0.0"}"#, "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).health()
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
}

#[tokio::test]
async fn explicit_limit_bounds_health_response_materialization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(r#"{"status":"ok","version":"1.0.0"}"#, "application/json"),
        )
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).health_with_limit(8)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result
        .expect_err("oversized health response must fail")
        .contains("8-byte materialized read limit"));
}

#[tokio::test]
async fn explicit_limit_bounds_get_metadata_response_materialization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/metadata/dlsite/RJ1"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"id":"RJ1","source":"dlsite","title":null,"creator":null,"description":null,"release_date":null,"tags":[],"extras":null}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None })
            .get_metadata_with_limit("dlsite", "RJ1", 8)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result
        .expect_err("oversized metadata response must fail")
        .contains("8-byte materialized read limit"));
}

#[tokio::test]
async fn explicit_limit_bounds_fetch_response_materialization() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/fetch"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"status":"ok","source":"dlsite","id":"RJ1","metadata":null}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None })
            .fetch_metadata_with_limit("dlsite", "RJ1", false, 8)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result
        .expect_err("oversized fetch response must fail")
        .contains("8-byte materialized read limit"));
}

#[tokio::test]
async fn explicit_limit_accepts_an_exact_fetch_response_boundary() {
    let server = MockServer::start().await;
    let body = r#"{"status":"ok","source":"dlsite","id":"RJ1","metadata":null}"#;
    let body_limit = body.len();
    Mock::given(method("POST"))
        .and(path("/api/v1/fetch"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None })
            .fetch_metadata_with_limit("dlsite", "RJ1", false, body_limit)
    })
    .await
    .expect("spawn_blocking panicked")
    .expect("exact response boundary should be accepted");

    assert_eq!(result.id, "RJ1");
}

#[tokio::test]
async fn explicit_limit_bounds_search_response_materialization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"query":"q","source":null,"results":[]}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        GametaClient::new(ServerConfig { url, api_key: None }).search_with_limit("q", None, None, 8)
    })
    .await
    .expect("spawn_blocking panicked");

    assert!(result
        .expect_err("oversized search response must fail")
        .contains("8-byte materialized read limit"));
}
