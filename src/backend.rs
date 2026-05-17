//! `ProviderBackend` impl that drives the Claude Code CLI via the native
//! session backend from `animus-session-backend`.

use std::path::Path;
use std::time::Instant;

use animus_plugin_protocol::{HealthCheckResult, HealthStatus};
use animus_provider_protocol::{
    AgentResumeRequest, AgentRunRequest, AgentRunResponse, BackendError, ProviderBackend,
    ProviderCapabilities, ProviderManifest, TokenUsage,
};
use animus_session_backend::{
    cli::lookup_binary_in_path, ClaudeSessionBackend, SessionBackend, SessionEvent, SessionRequest,
    SessionRun,
};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::ClaudeConfig;

/// Provider backend wired to a [`SessionBackend`] (defaults to the native
/// Claude Code session backend).
///
/// The session backend is left generic so tests can inject a deterministic
/// fake without spawning a real `claude` child process.
pub struct ClaudeProviderBackend<S: SessionBackend = ClaudeSessionBackend> {
    session: S,
    config: ClaudeConfig,
}

impl ClaudeProviderBackend<ClaudeSessionBackend> {
    /// Construct the production backend wired to the native CLI session
    /// backend.
    pub fn new(config: ClaudeConfig) -> Self {
        Self {
            session: ClaudeSessionBackend::new(),
            config,
        }
    }
}

impl<S: SessionBackend> ClaudeProviderBackend<S> {
    /// Construct a backend with a caller-supplied session implementation.
    ///
    /// Used by integration tests to inject a fake `SessionBackend`.
    pub fn with_session(session: S, config: ClaudeConfig) -> Self {
        Self { session, config }
    }

    /// Read-only access to the active config (handy for tests).
    pub fn config(&self) -> &ClaudeConfig {
        &self.config
    }
}

#[async_trait]
impl<S: SessionBackend + 'static> ProviderBackend for ClaudeProviderBackend<S> {
    fn manifest(&self) -> ProviderManifest {
        let caps = self.session.capabilities();
        ProviderManifest {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: env!("CARGO_PKG_DESCRIPTION").to_string(),
            supported_models: vec![
                "claude-sonnet-4-6".to_string(),
                "claude-opus-4-7".to_string(),
                "claude-haiku-4-5".to_string(),
                "claude-3-5-sonnet-latest".to_string(),
                "claude-3-5-haiku-latest".to_string(),
            ],
            tool: "claude".to_string(),
            capabilities: ProviderCapabilities {
                streaming: true,
                resume: caps.supports_resume,
                cancellation: caps.supports_terminate,
                write_capable: true,
                mcp: caps.supports_mcp,
            },
        }
    }

    async fn run_agent(&self, request: AgentRunRequest) -> Result<AgentRunResponse, BackendError> {
        let started = Instant::now();
        let session_request = translate_to_session_request(&request, &self.config);
        let run = self
            .session
            .start_session(session_request)
            .await
            .map_err(|e| BackendError::SessionStartFailed(format!("claude session: {e}")))?;
        Ok(drain_session_run(run, started, request.model.as_deref(), &self.config).await)
    }

    async fn resume_agent(
        &self,
        request: AgentResumeRequest,
    ) -> Result<AgentRunResponse, BackendError> {
        let started = Instant::now();
        let session_id = request
            .session_id
            .clone()
            .ok_or_else(|| BackendError::Other(anyhow::anyhow!("resume requires session_id")))?;
        let session_request = translate_to_session_request(&request, &self.config);
        let run = self
            .session
            .resume_session(session_request, &session_id)
            .await
            .map_err(|e| BackendError::SessionStartFailed(format!("claude resume: {e}")))?;
        Ok(drain_session_run(run, started, request.model.as_deref(), &self.config).await)
    }

    async fn cancel_agent(&self, session_id: &str) -> Result<(), BackendError> {
        self.session
            .terminate_session(session_id)
            .await
            .map_err(|e| BackendError::Other(anyhow::anyhow!("claude cancel: {e}")))
    }

    async fn health(&self) -> Result<HealthCheckResult, BackendError> {
        let bin = &self.config.claude_bin;
        let healthy = lookup_binary_in_path(bin).is_some() || Path::new(bin).exists();
        Ok(HealthCheckResult {
            status: if healthy {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy
            },
            uptime_ms: None,
            memory_usage_bytes: None,
            last_error: if healthy {
                None
            } else {
                Some(format!("binary `{bin}` not found on PATH"))
            },
        })
    }
}

/// Translate the protocol-level `AgentRunRequest` (or resume request) into the
/// `SessionRequest` shape the session backend expects.
fn translate_to_session_request(
    request: &AgentRunRequest,
    config: &ClaudeConfig,
) -> SessionRequest {
    let model = request
        .model
        .clone()
        .unwrap_or_else(|| config.default_model.clone());

    let mcp_endpoint = mcp_endpoint_from_servers(request.mcp_servers.as_ref());

    let env_vars: Vec<(String, String)> = request
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let mut extras_map = serde_json::Map::new();
    if let Some(rc) = &request.runtime_contract {
        extras_map.insert("runtime_contract".to_string(), rc.clone());
    }
    if let Some(system) = &request.system_prompt {
        extras_map.insert("system_prompt".to_string(), Value::String(system.clone()));
    }
    if let Some(tools) = &request.tools {
        extras_map.insert("tools".to_string(), tools.clone());
    }
    if let Some(schema) = &request.response_schema {
        extras_map.insert("response_schema".to_string(), schema.clone());
    }
    for (k, v) in &request.extras {
        extras_map.insert(k.clone(), v.clone());
    }

    SessionRequest {
        tool: "claude".to_string(),
        model,
        prompt: request.prompt.clone(),
        cwd: request.cwd.clone(),
        project_root: request.project_root.clone(),
        mcp_endpoint,
        permission_mode: request.permission_mode.clone(),
        timeout_secs: request.timeout_secs,
        env_vars,
        extras: Value::Object(extras_map),
    }
}

fn mcp_endpoint_from_servers(mcp_servers: Option<&Value>) -> Option<String> {
    let value = mcp_servers?;
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    if let Some(s) = value.get("endpoint").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = value.get("url").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    None
}

/// Drain the session event channel into the aggregated `AgentRunResponse`.
///
/// For v0.1.0 this is fully synchronous (we wait for `Finished` before
/// returning). A future iteration can wire `SessionEvent::TextDelta` events
/// through to JSON-RPC `agent/output` notifications via the plugin-runtime
/// emitter once that surface is stable.
async fn drain_session_run(
    mut run: SessionRun,
    started: Instant,
    requested_model: Option<&str>,
    config: &ClaudeConfig,
) -> AgentRunResponse {
    let backend_label = format!("claude:{}", run.selected_backend);
    let mut session_id = run.session_id.clone();

    let mut output_text = String::new();
    let mut final_text: Option<String> = None;
    let mut thinking: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_results: Vec<Value> = Vec::new();
    let mut metadata: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut tokens: Option<TokenUsage> = None;
    let mut exit_code: i32 = 0;

    while let Some(event) = run.events.recv().await {
        match event {
            SessionEvent::Started {
                session_id: started_id,
                ..
            } => {
                if session_id.is_none() {
                    session_id = started_id;
                }
            }
            SessionEvent::TextDelta { text } => {
                output_text.push_str(&text);
            }
            SessionEvent::FinalText { text } => {
                final_text = Some(text);
            }
            SessionEvent::ToolCall {
                tool_name,
                arguments,
                server,
            } => {
                tool_calls.push(json!({
                    "tool": tool_name,
                    "arguments": arguments,
                    "server": server,
                }));
            }
            SessionEvent::ToolResult {
                tool_name,
                output,
                success,
            } => {
                tool_results.push(json!({
                    "tool": tool_name,
                    "output": output,
                    "success": success,
                }));
            }
            SessionEvent::Thinking { text } => thinking.push(text),
            SessionEvent::Artifact {
                artifact_id,
                metadata: meta,
            } => {
                metadata.push(json!({
                    "kind": "artifact",
                    "artifact_id": artifact_id,
                    "metadata": meta,
                }));
            }
            SessionEvent::Metadata { metadata: meta } => {
                if tokens.is_none() {
                    tokens = parse_token_usage(&meta);
                }
                metadata.push(meta);
            }
            SessionEvent::Error {
                message,
                recoverable,
            } => {
                errors.push(message.clone());
                if !recoverable {
                    exit_code = 1;
                }
            }
            SessionEvent::Finished { exit_code: code } => {
                if let Some(code) = code {
                    exit_code = code;
                }
                break;
            }
        }
    }

    let output = final_text.unwrap_or(output_text);
    let session_id = session_id.unwrap_or_default();

    let _ = (requested_model, &config.default_model);

    AgentRunResponse {
        session_id,
        exit_code,
        output,
        metadata,
        tool_calls,
        tool_results,
        thinking,
        errors,
        duration_ms: started.elapsed().as_millis() as u64,
        backend: backend_label,
        tokens_used: tokens,
        decision_verdict: None,
    }
}

/// Best-effort parse of a token-usage frame coming from the Claude CLI's
/// `result` event. Looks at the conventional Anthropic field names.
fn parse_token_usage(meta: &Value) -> Option<TokenUsage> {
    let usage = meta.get("usage").or_else(|| meta.get("token_usage"))?;
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("input"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("output"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.get("cached"))
        .and_then(|v| v.as_u64());
    let cache_writes = usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.get("cache_writes"))
        .and_then(|v| v.as_u64());

    if input == 0 && output == 0 && cached.is_none() && cache_writes.is_none() {
        return None;
    }
    Some(TokenUsage {
        input,
        output,
        cached,
        cache_writes,
    })
}
