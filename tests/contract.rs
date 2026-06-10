use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use animus_plugin_protocol::HealthStatus;
use animus_provider_claude::backend::ClaudeProviderBackend;
use animus_provider_claude::config::ClaudeConfig;
use animus_provider_protocol::{
    AgentNotification, AgentRunRequest, NotificationSink, ProviderBackend,
};
use animus_session_backend::{
    Error as SessionError, Result as SessionResult, SessionBackend, SessionBackendInfo,
    SessionBackendKind, SessionCapabilities, SessionEvent, SessionRequest, SessionRun,
    SessionStability,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

/// Canned script the fake session backend replays into the event channel.
#[derive(Default, Clone)]
struct FakeScript {
    session_id: Option<String>,
    events: Vec<SessionEvent>,
    selected_backend: String,
}

/// Records every call the provider made so tests can assert on parameter
/// passthrough.
#[derive(Default)]
struct FakeCalls {
    starts: Vec<SessionRequest>,
    resumes: Vec<(SessionRequest, String)>,
    terminations: Vec<String>,
}

struct FakeSession {
    script: FakeScript,
    calls: Arc<Mutex<FakeCalls>>,
    capabilities: SessionCapabilities,
}

impl FakeSession {
    fn new(script: FakeScript) -> Self {
        Self {
            script,
            calls: Arc::new(Mutex::new(FakeCalls::default())),
            capabilities: SessionCapabilities {
                supports_resume: true,
                supports_terminate: true,
                supports_permissions: true,
                supports_mcp: true,
                supports_tool_events: true,
                supports_thinking_events: true,
                supports_artifact_events: false,
                supports_usage_metadata: true,
            },
        }
    }

    fn build_run(&self) -> SessionRun {
        let (tx, rx) = mpsc::channel(64);
        for event in self.script.events.clone() {
            tx.try_send(event).expect("fake channel buffer big enough");
        }
        drop(tx);
        SessionRun {
            session_id: self.script.session_id.clone(),
            events: rx,
            selected_backend: self.script.selected_backend.clone(),
            fallback_reason: None,
            pid: None,
        }
    }
}

#[async_trait]
impl SessionBackend for FakeSession {
    fn info(&self) -> SessionBackendInfo {
        SessionBackendInfo {
            kind: SessionBackendKind::ClaudeSdk,
            provider_tool: "claude".to_string(),
            stability: SessionStability::Experimental,
            display_name: "Fake Claude Backend".to_string(),
        }
    }

    fn capabilities(&self) -> SessionCapabilities {
        self.capabilities.clone()
    }

    async fn start_session(&self, request: SessionRequest) -> SessionResult<SessionRun> {
        self.calls.lock().unwrap().starts.push(request);
        Ok(self.build_run())
    }

    async fn resume_session(
        &self,
        request: SessionRequest,
        session_id: &str,
    ) -> SessionResult<SessionRun> {
        self.calls
            .lock()
            .unwrap()
            .resumes
            .push((request, session_id.to_string()));
        Ok(self.build_run())
    }

    async fn terminate_session(&self, session_id: &str) -> SessionResult<()> {
        self.calls
            .lock()
            .unwrap()
            .terminations
            .push(session_id.to_string());
        Ok(())
    }
}

/// Fake that fails `terminate_session` so we can exercise the error path.
struct FailingTerminateFake;

#[async_trait]
impl SessionBackend for FailingTerminateFake {
    fn info(&self) -> SessionBackendInfo {
        SessionBackendInfo {
            kind: SessionBackendKind::ClaudeSdk,
            provider_tool: "claude".to_string(),
            stability: SessionStability::Experimental,
            display_name: "Failing Fake".to_string(),
        }
    }

    fn capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            supports_resume: false,
            supports_terminate: false,
            supports_permissions: false,
            supports_mcp: false,
            supports_tool_events: false,
            supports_thinking_events: false,
            supports_artifact_events: false,
            supports_usage_metadata: false,
        }
    }

    async fn start_session(&self, _: SessionRequest) -> SessionResult<SessionRun> {
        Err(SessionError::ExecutionFailed("disabled".to_string()))
    }

    async fn resume_session(&self, _: SessionRequest, _: &str) -> SessionResult<SessionRun> {
        Err(SessionError::ExecutionFailed("disabled".to_string()))
    }

    async fn terminate_session(&self, _: &str) -> SessionResult<()> {
        Err(SessionError::ExecutionFailed(
            "session not tracked".to_string(),
        ))
    }
}

fn make_request(model: Option<&str>, prompt: &str) -> AgentRunRequest {
    AgentRunRequest {
        session_id: None,
        prompt: prompt.to_string(),
        model: model.map(|s| s.to_string()),
        system_prompt: Some("you are a helpful coding agent".to_string()),
        cwd: PathBuf::from("/tmp/cwd"),
        project_root: Some(PathBuf::from("/tmp/cwd")),
        permission_mode: Some("acceptEdits".to_string()),
        timeout_secs: Some(60),
        env: HashMap::new(),
        mcp_servers: None,
        tools: None,
        response_schema: None,
        runtime_contract: None,
        extras: HashMap::new(),
    }
}

#[tokio::test]
async fn run_agent_aggregates_text_deltas_and_metadata() {
    let script = FakeScript {
        session_id: Some("sess-abc".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![
            SessionEvent::Started {
                backend: "fake-claude".to_string(),
                session_id: Some("sess-abc".to_string()),
                pid: Some(4242),
            },
            SessionEvent::TextDelta {
                text: "hello ".to_string(),
            },
            SessionEvent::TextDelta {
                text: "world".to_string(),
            },
            SessionEvent::Thinking {
                text: "let me think".to_string(),
            },
            SessionEvent::ToolCall {
                tool_name: "Read".to_string(),
                arguments: json!({"path": "/tmp/foo"}),
                server: None,
            },
            SessionEvent::ToolResult {
                tool_name: "Read".to_string(),
                output: json!("ok"),
                success: true,
            },
            SessionEvent::Metadata {
                metadata: json!({
                    "usage": {
                        "input_tokens": 12,
                        "output_tokens": 7,
                        "cache_read_input_tokens": 3
                    }
                }),
            },
            SessionEvent::Finished { exit_code: Some(0) },
        ],
    };
    let fake = FakeSession::new(script);
    let backend = ClaudeProviderBackend::with_session(fake, ClaudeConfig::default());

    let response = backend
        .run_agent(make_request(Some("claude-sonnet-4-6"), "say hi"))
        .await
        .expect("run_agent should succeed");

    assert_eq!(response.session_id, "sess-abc");
    assert_eq!(response.exit_code, 0);
    assert_eq!(response.output, "hello world");
    assert_eq!(response.thinking, vec!["let me think".to_string()]);
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_results.len(), 1);
    assert!(response.backend.starts_with("claude:"));
    let usage = response.tokens_used.expect("usage propagated");
    assert_eq!(usage.input, 12);
    assert_eq!(usage.output, 7);
    assert_eq!(usage.cached, Some(3));
}

#[tokio::test]
async fn run_agent_prefers_final_text_over_deltas() {
    let script = FakeScript {
        session_id: Some("sess-def".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![
            SessionEvent::TextDelta {
                text: "partial".to_string(),
            },
            SessionEvent::FinalText {
                text: "final answer".to_string(),
            },
            SessionEvent::Finished { exit_code: Some(0) },
        ],
    };
    let backend =
        ClaudeProviderBackend::with_session(FakeSession::new(script), ClaudeConfig::default());

    let response = backend
        .run_agent(make_request(None, "go"))
        .await
        .expect("run_agent succeeds");
    assert_eq!(response.output, "final answer");
}

#[tokio::test]
async fn resume_agent_forwards_session_id_to_backend() {
    let script = FakeScript {
        session_id: Some("sess-resumed".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![
            SessionEvent::FinalText {
                text: "resumed!".to_string(),
            },
            SessionEvent::Finished { exit_code: Some(0) },
        ],
    };
    let fake = FakeSession::new(script);
    let backend = ClaudeProviderBackend::with_session(fake, ClaudeConfig::default());

    let mut request = make_request(Some("claude-sonnet-4-6"), "keep going");
    request.session_id = Some("sess-prior".to_string());

    let response = backend
        .resume_agent(request)
        .await
        .expect("resume_agent succeeds");
    assert_eq!(response.output, "resumed!");
}

#[tokio::test]
async fn resume_agent_requires_session_id() {
    let script = FakeScript {
        session_id: None,
        selected_backend: "fake-claude".to_string(),
        events: vec![SessionEvent::Finished { exit_code: Some(0) }],
    };
    let backend =
        ClaudeProviderBackend::with_session(FakeSession::new(script), ClaudeConfig::default());

    let request = make_request(None, "no session id");
    let err = backend
        .resume_agent(request)
        .await
        .expect_err("resume should require a session id");
    assert!(
        format!("{err}").contains("session_id"),
        "error mentions session_id: {err}"
    );
}

#[tokio::test]
async fn cancel_agent_invokes_terminate_session() {
    let script = FakeScript {
        session_id: Some("sess-zzz".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![SessionEvent::Finished { exit_code: Some(0) }],
    };
    let backend =
        ClaudeProviderBackend::with_session(FakeSession::new(script), ClaudeConfig::default());

    backend
        .cancel_agent("sess-zzz")
        .await
        .expect("cancel should succeed against the fake");
}

#[tokio::test]
async fn cancel_agent_surfaces_backend_errors() {
    let backend =
        ClaudeProviderBackend::with_session(FailingTerminateFake, ClaudeConfig::default());

    let err = backend
        .cancel_agent("sess-unknown")
        .await
        .expect_err("cancel should propagate the backend failure");
    assert!(format!("{err}").contains("cancel"));
}

#[tokio::test]
async fn manifest_has_expected_capabilities() {
    let script = FakeScript {
        session_id: None,
        selected_backend: "fake-claude".to_string(),
        events: vec![],
    };
    let backend =
        ClaudeProviderBackend::with_session(FakeSession::new(script), ClaudeConfig::default());
    let manifest = backend.manifest();
    assert_eq!(manifest.name, "animus-provider-claude");
    assert_eq!(manifest.tool, "claude");
    assert!(manifest.capabilities.streaming);
    assert!(manifest.capabilities.resume);
    assert!(manifest.capabilities.cancellation);
    assert!(manifest.capabilities.write_capable);
    assert!(manifest.capabilities.mcp);
    assert!(manifest
        .supported_models
        .iter()
        .any(|m| m == "claude-sonnet-4-6"));
}

#[tokio::test]
async fn health_unhealthy_when_binary_missing() {
    let config = ClaudeConfig::for_testing(
        "/definitely/not/a/real/path/to/claude-xyzzy",
        "claude-sonnet-4-6",
    );
    let backend = ClaudeProviderBackend::new(config);
    let health = backend.health().await.expect("health does not error");
    assert_eq!(health.status, HealthStatus::Unhealthy);
    assert!(health.last_error.is_some());
}

#[tokio::test]
async fn run_agent_streaming_emits_notifications_in_event_order() {
    let script = FakeScript {
        session_id: Some("sess-stream".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![
            SessionEvent::Started {
                backend: "fake-claude".to_string(),
                session_id: Some("sess-stream".to_string()),
                pid: Some(7777),
            },
            SessionEvent::TextDelta {
                text: "hi ".to_string(),
            },
            SessionEvent::Thinking {
                text: "pondering".to_string(),
            },
            SessionEvent::TextDelta {
                text: "there".to_string(),
            },
            SessionEvent::ToolCall {
                tool_name: "Read".to_string(),
                arguments: json!({"path": "/tmp/x"}),
                server: Some("local".to_string()),
            },
            SessionEvent::ToolResult {
                tool_name: "Read".to_string(),
                output: json!({"bytes": 42}),
                success: true,
            },
            SessionEvent::Error {
                message: "transient blip".to_string(),
                recoverable: true,
            },
            SessionEvent::FinalText {
                text: "hi there".to_string(),
            },
            SessionEvent::Finished { exit_code: Some(0) },
        ],
    };
    let backend =
        ClaudeProviderBackend::with_session(FakeSession::new(script), ClaudeConfig::default());

    let recorder: Arc<Mutex<Vec<AgentNotification>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = Arc::clone(&recorder);
    let sink = NotificationSink::new(move |n| r2.lock().unwrap().push(n));

    let response = backend
        .run_agent_streaming(
            make_request(Some("claude-sonnet-4-6"), "stream please"),
            sink,
        )
        .await
        .expect("streaming run succeeds");

    assert_eq!(response.session_id, "sess-stream");
    assert_eq!(response.output, "hi there");
    assert_eq!(response.exit_code, 0);

    let notifications = recorder.lock().unwrap().clone();
    assert_eq!(
        notifications.len(),
        7,
        "expected 2 deltas + 1 thinking + 1 tool call + 1 tool result + 1 error + 1 final = 7, got {notifications:?}"
    );

    match &notifications[0] {
        AgentNotification::Output {
            session_id,
            text,
            is_final,
        } => {
            assert_eq!(session_id, "sess-stream");
            assert_eq!(text, "hi ");
            assert!(!is_final);
        }
        other => panic!("expected Output, got {other:?}"),
    }
    match &notifications[1] {
        AgentNotification::Thinking { session_id, text } => {
            assert_eq!(session_id, "sess-stream");
            assert_eq!(text, "pondering");
        }
        other => panic!("expected Thinking, got {other:?}"),
    }
    match &notifications[2] {
        AgentNotification::Output { text, is_final, .. } => {
            assert_eq!(text, "there");
            assert!(!is_final);
        }
        other => panic!("expected Output, got {other:?}"),
    }
    match &notifications[3] {
        AgentNotification::ToolCall {
            name,
            arguments,
            server,
            ..
        } => {
            assert_eq!(name, "Read");
            assert_eq!(arguments, &json!({"path": "/tmp/x"}));
            assert_eq!(server.as_deref(), Some("local"));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
    match &notifications[4] {
        AgentNotification::ToolResult {
            name,
            output,
            success,
            ..
        } => {
            assert_eq!(name, "Read");
            assert_eq!(output, &json!({"bytes": 42}));
            assert!(*success);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    match &notifications[5] {
        AgentNotification::Error {
            message,
            recoverable,
            ..
        } => {
            assert_eq!(message, "transient blip");
            assert!(*recoverable);
        }
        other => panic!("expected Error, got {other:?}"),
    }
    match &notifications[6] {
        AgentNotification::Output { text, is_final, .. } => {
            assert_eq!(text, "hi there");
            assert!(*is_final);
        }
        other => panic!("expected final Output, got {other:?}"),
    }
}

#[tokio::test]
async fn run_agent_noop_sink_path_matches_streaming_response() {
    let make_script = || FakeScript {
        session_id: Some("sess-eq".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![
            SessionEvent::Started {
                backend: "fake-claude".to_string(),
                session_id: Some("sess-eq".to_string()),
                pid: None,
            },
            SessionEvent::TextDelta {
                text: "abc".to_string(),
            },
            SessionEvent::FinalText {
                text: "abc!".to_string(),
            },
            SessionEvent::Finished { exit_code: Some(0) },
        ],
    };

    let bulk_backend = ClaudeProviderBackend::with_session(
        FakeSession::new(make_script()),
        ClaudeConfig::default(),
    );
    let bulk = bulk_backend
        .run_agent(make_request(None, "x"))
        .await
        .expect("bulk run");

    let stream_backend = ClaudeProviderBackend::with_session(
        FakeSession::new(make_script()),
        ClaudeConfig::default(),
    );
    let stream = stream_backend
        .run_agent_streaming(make_request(None, "x"), NotificationSink::noop())
        .await
        .expect("stream run");

    assert_eq!(bulk.session_id, stream.session_id);
    assert_eq!(bulk.output, stream.output);
    assert_eq!(bulk.exit_code, stream.exit_code);
    assert_eq!(bulk.tool_calls, stream.tool_calls);
    assert_eq!(bulk.tool_results, stream.tool_results);
    assert_eq!(bulk.thinking, stream.thinking);
    assert_eq!(bulk.errors, stream.errors);
}

#[tokio::test]
async fn health_healthy_when_binary_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_path = dir.path().join("claude-test-stub");
    std::fs::write(&bin_path, b"#!/bin/sh\nexit 0\n").expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }

    let config =
        ClaudeConfig::for_testing(bin_path.to_string_lossy().to_string(), "claude-sonnet-4-6");
    let backend = ClaudeProviderBackend::new(config);
    let health = backend.health().await.expect("health does not error");
    assert_eq!(health.status, HealthStatus::Healthy);
    assert!(health.last_error.is_none());
}

#[tokio::test]
async fn run_agent_passes_mcp_servers_through_to_session_request() {
    let script = FakeScript {
        session_id: Some("sess-mcp".to_string()),
        selected_backend: "fake-claude".to_string(),
        events: vec![
            SessionEvent::FinalText {
                text: "done".to_string(),
            },
            SessionEvent::Finished { exit_code: Some(0) },
        ],
    };
    let fake = FakeSession::new(script);
    let calls = Arc::clone(&fake.calls);
    let backend = ClaudeProviderBackend::with_session(fake, ClaudeConfig::default());

    let servers = json!({
        "docs": { "command": "npx", "args": ["-y", "docs-mcp"] },
        "linear": { "type": "http", "url": "https://mcp.linear.app/mcp" }
    });
    let mut request = make_request(None, "go");
    request.mcp_servers = Some(servers.clone());

    backend
        .run_agent(request)
        .await
        .expect("run_agent succeeds");

    let calls = calls.lock().unwrap();
    assert_eq!(calls.starts.len(), 1);
    assert_eq!(calls.starts[0].mcp_servers, Some(servers));
    assert_eq!(calls.starts[0].mcp_endpoint, None);
}
