use super::*;

#[test]
fn test_rate_limiter() {
    let limiter = RateLimiter::new(5);

    // Should allow first 5 requests
    for _ in 0..5 {
        assert!(limiter.check_rate_limit());
    }

    // Should deny 6th request
    assert!(!limiter.check_rate_limit());
}

#[test]
fn test_host_functions_creation() {
    let caps = vec![PluginCapability::Network].into_iter().collect();
    let host_funcs = HostFunctions::new(caps, 10);

    assert!(host_funcs.http_client.is_some());
    assert!(host_funcs.check_capability(PluginCapability::Network));
    assert!(!host_funcs.check_capability(PluginCapability::FileRead));
}

#[test]
fn test_buffer_allocation() {
    let caps = std::collections::HashSet::new();
    let host_funcs = HostFunctions::new(caps, 10);

    let data = vec![1, 2, 3, 4, 5];
    let id = host_funcs.allocate_buffer(data.clone());

    let retrieved = host_funcs.take_buffer(id);
    assert_eq!(retrieved, Some(data));

    // Should be removed after taking
    assert_eq!(host_funcs.take_buffer(id), None);
}

#[test]
fn test_capability_checking() {
    let caps = vec![PluginCapability::FileRead, PluginCapability::Network]
        .into_iter()
        .collect();

    let host_funcs = HostFunctions::new(caps, 10);

    assert!(host_funcs.check_capability(PluginCapability::FileRead));
    assert!(host_funcs.check_capability(PluginCapability::Network));
    assert!(!host_funcs.check_capability(PluginCapability::FileWrite));
    assert!(!host_funcs.check_capability(PluginCapability::ArchiveMetadataWrite));
}
