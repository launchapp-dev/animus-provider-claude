# animus-provider-claude

A [Claude Code](https://docs.anthropic.com/en/docs/claude-code) provider plugin for [Animus](https://github.com/launchapp-dev/animus-cli).

> **Status:** Under construction — landing in Animus v0.4.x. This crate currently lives in the Animus core workspace at `crates/animus-provider-claude/`; v0.4.x extracts it to this standalone repository.

## What this is

Animus v0.4.0 makes providers (LLM CLI wrappers) pluggable. This repository will ship `animus-provider-claude`, a stdio plugin that wraps Anthropic's Claude Code CLI as an Animus provider. Any workflow phase that targets `tool: claude` dispatches through this plugin.

## Install (planned)

Once published:

```bash
animus plugin install animus-provider-claude
```

The Animus daemon image bundles this plugin pre-installed, so `tool: claude` workflows work out of the box on hosted runners.

## Workflow YAML usage (no change from v0.3.x)

```yaml
agents:
  default:
    model: claude-sonnet-4-6
    tool: claude
    mcp_servers: ["animus"]
```

The `tool: claude` line resolves to this provider plugin via the daemon's plugin registry.

## Roadmap

- [ ] Extract from Animus core workspace at v0.4.x cut
- [ ] Publish `animus-provider-claude` crate to crates.io
- [ ] Release binaries (macOS aarch64/x86_64, Linux x86_64) on tag
- [ ] Independent semver track
- [ ] CI exercises the contract test from `animus-protocol`

## Design

- **Protocol:** [`animus-plugin-protocol`](https://github.com/launchapp-dev/animus-protocol) (provider variant)
- **Naming:** repo, crate, and binary all named `animus-provider-claude` per the [naming contract](https://github.com/launchapp-dev/animus-cli/blob/main/docs/architecture/naming-contract.md)
- **Core repo:** [Animus](https://github.com/launchapp-dev/animus-cli)

## License

MIT — see [LICENSE](LICENSE).
