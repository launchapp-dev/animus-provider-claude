use anyhow::Result;

/// Runtime configuration for the Claude provider plugin.
///
/// Populated via environment variables in production; tests can construct it
/// directly through the public fields.
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// Path or name of the Claude CLI binary the plugin shells out to.
    /// Defaults to `"claude"` (resolved on PATH).
    pub claude_bin: String,
    /// Default model id passed through when an `agent/run` request omits
    /// `model`. Defaults to `"claude-sonnet-4-6"`.
    pub default_model: String,
}

impl ClaudeConfig {
    /// Load config from process environment.
    ///
    /// - `CLAUDE_BIN` — override CLI binary (default `"claude"`).
    /// - `CLAUDE_DEFAULT_MODEL` — override fallback model (default
    ///   `"claude-sonnet-4-6"`).
    pub fn from_env() -> Result<Self> {
        let claude_bin = std::env::var("CLAUDE_BIN")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "claude".to_string());
        let default_model = std::env::var("CLAUDE_DEFAULT_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string());
        Ok(Self {
            claude_bin,
            default_model,
        })
    }

    /// Test helper — construct a config with explicit values.
    pub fn for_testing(claude_bin: impl Into<String>, default_model: impl Into<String>) -> Self {
        Self {
            claude_bin: claude_bin.into(),
            default_model: default_model.into(),
        }
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            claude_bin: "claude".to_string(),
            default_model: "claude-sonnet-4-6".to_string(),
        }
    }
}
