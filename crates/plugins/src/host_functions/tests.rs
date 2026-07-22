use super::*;

fn host_functions(
    plugin_id: &str,
    capabilities: std::collections::HashSet<PluginCapability>,
    requests_per_minute: u32,
) -> HostFunctions {
    HostFunctions::new(
        plugin_id.to_string(),
        capabilities,
        requests_per_minute,
        HashMap::new(),
    )
    .unwrap()
}

fn async_client(runtime: &tokio::runtime::Runtime) -> Arc<arclain_network::AsyncHttpClient> {
    Arc::new(arclain_network::AsyncHttpClient::new(
        runtime.handle().clone(),
        Arc::new(parking_lot::RwLock::new(
            arclain_network::DomainWhitelist::default(),
        )),
        None,
    ))
}

#[test]
fn async_client_observes_exact_manifest_network_policy() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = async_client(&runtime);
    let capabilities = [PluginCapability::Network].into_iter().collect();
    let mut host = host_functions("manifest-policy", capabilities, 7);

    host.set_async_http_client(client.clone());

    assert_eq!(
        client.plugin_network_policy("manifest-policy"),
        Some(arclain_network::PluginNetworkPolicy {
            network_enabled: true,
            requests_per_minute: 7,
        })
    );
}

#[test]
fn disabled_manifest_network_capability_is_registered_disabled() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = async_client(&runtime);
    let mut host = host_functions("disabled-policy", Default::default(), 19);

    host.set_async_http_client(client.clone());

    assert_eq!(
        client.plugin_network_policy("disabled-policy"),
        Some(arclain_network::PluginNetworkPolicy {
            network_enabled: false,
            requests_per_minute: 19,
        })
    );
    assert!(matches!(
        client.request_for_plugin(
            "disabled-policy",
            arclain_network::HttpRequest::get("https://example.com/"),
        ),
        Err(arclain_network::HttpError::PluginNetworkDisabled { .. })
    ));
}
