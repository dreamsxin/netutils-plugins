# netutils plugins

Official external plugins for `netutils-cli`.

The core CLI discovers plugins as external subcommands. For example:

```bash
netutils install mcp
netutils mcp https://example.com/mcp
netutils mcp https://example.com/mcp --tool tabs --args '{"action":"list"}'
```

During local development, install a plugin from a local checkout:

```bash
cd <path-to-netutils-cli>
cargo run -- install mcp --path <path-to-netutils-plugins>/plugins/mcp --force
cargo run -- mcp https://example.com/mcp
```

See [PLUGIN_DEVELOPMENT.md](PLUGIN_DEVELOPMENT.md) for plugin layout, command conventions, testing, and release guidance.

## Plugins

| Plugin | Binary | Description |
|--------|--------|-------------|
| `mcp` | `netutils-mcp` | MCP Streamable HTTP diagnostics |

## MCP plugin

The MCP plugin tests Streamable HTTP servers by running:

- `initialize`
- `notifications/initialized`
- `tools/list` by default
- optional `tools/call`
- optional GET server-to-client SSE listen stream

Examples:

```bash
netutils mcp https://example.com/mcp
netutils mcp https://example.com/mcp -H "Authorization: Bearer xxx"
netutils mcp https://example.com/mcp --protocol-version 2025-11-25 --listen
netutils mcp https://example.com/mcp --tool tabs --args '{"action":"list"}'
netutils mcp https://example.com/mcp --tool search --args '{"query":"netutils"}' --require-tool
```

Options for calling a tool:

| Option | Description |
|--------|-------------|
| `--tool <NAME>` | Calls one MCP tool with `tools/call` after initialization |
| `--args <JSON>` | JSON object passed as `params.arguments`; defaults to `{}` |
| `--require-tool` | Calls the tool only if it appears in `tools/list` |
| `--no-tools` | Skips `tools/list`; cannot be combined meaningfully with `--require-tool` |

The plugin accepts both `application/json` and `text/event-stream` JSON-RPC responses. JSON output includes the raw `tools/call` response under `tool_call`.
