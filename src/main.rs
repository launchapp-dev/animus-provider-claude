use animus_plugin_protocol::{PluginInfo, PLUGIN_KIND_PROVIDER};
use animus_plugin_runtime::provider_main_with_capabilities;
use animus_provider_claude::backend::ClaudeProviderBackend;
use animus_provider_claude::config::ClaudeConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    emit_manifest_if_requested();

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

fn emit_manifest_if_requested() {
    if !std::env::args()
        .skip(1)
        .any(|arg| arg == "--manifest" || arg == "-m")
    {
        return;
    }

    let manifest = serde_json::json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "plugin_kind": "provider",
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "protocol_version": animus_plugin_protocol::PROTOCOL_VERSION,
        "capabilities": [
            "agent/run",
            "agent/resume",
            "agent/cancel",
            "health/check",
            "$harness/cancellation-loop-v2"
        ],
        "env_required": [
            {
                "name": "CLAUDE_BIN",
                "description": "Override the Claude CLI binary path.",
                "required": false
            },
            {
                "name": "CLAUDE_DEFAULT_MODEL",
                "description": "Fallback model used when the request omits a model.",
                "required": false
            },
            {
                "name": "ANTHROPIC_API_KEY",
                "description": "Anthropic API key used by Claude CLI when API-key auth is configured.",
                "sensitive": true,
                "required": false
            },
            {
                "name": "ANTHROPIC_AUTH_TOKEN",
                "description": "Anthropic auth token used by Claude CLI in some auth flows.",
                "sensitive": true,
                "required": false
            },
            {
                "name": "ANTHROPIC_BASE_URL",
                "description": "Override the Anthropic API base URL.",
                "required": false
            },
            {
                "name": "CLAUDE_CONFIG_DIR",
                "description": "Override the Claude CLI config directory.",
                "required": false
            }
        ]
    });
    println!(
        "{}",
        serde_json::to_string(&manifest).expect("serialize manifest")
    );
    std::process::exit(0);
}
