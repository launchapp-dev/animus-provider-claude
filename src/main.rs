use animus_plugin_protocol::{PluginInfo, PLUGIN_KIND_PROVIDER};
use animus_plugin_runtime::provider_main_with_capabilities;
use animus_provider_claude::backend::ClaudeProviderBackend;
use animus_provider_claude::config::ClaudeConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let config = ClaudeConfig::from_env()?;
    let backend = ClaudeProviderBackend::new(config);

    let info = PluginInfo {
        name: env!("CARGO_PKG_NAME").into(),
        version: env!("CARGO_PKG_VERSION").into(),
        plugin_kind: PLUGIN_KIND_PROVIDER.into(),
        description: Some(env!("CARGO_PKG_DESCRIPTION").into()),
    };

    // claude supports mid-flight cancel: backend.cancel_agent terminates
    // the underlying `claude` CLI subprocess via the session manager.
    // Opt in to the testkit's concurrent-cancel conformance scenario.
    let extra_capabilities = vec!["$harness/cancellation-loop-v2".to_string()];

    provider_main_with_capabilities(info, backend, extra_capabilities).await
}
