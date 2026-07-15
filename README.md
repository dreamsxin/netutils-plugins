# netutils plugins

Official external plugins for `netutils-cli`.

The core CLI discovers plugins as external subcommands. For example:

```bash
netutils install mcp
netutils mcp https://example.com/mcp
netutils mcp https://example.com/mcp --tool tabs --args '{"action":"list"}'
netutils install sse
netutils sse https://example.com/events
netutils install ws
netutils ws wss://echo.websocket.events --message ping
netutils install subdomain
netutils subdomain example.com
```

During local development, install a plugin from a local checkout:

```bash
cd <path-to-netutils-cli>
cargo run -- install mcp --path <path-to-netutils-plugins>/plugins/mcp --force
cargo run -- mcp https://example.com/mcp
cargo run -- install sse --path <path-to-netutils-plugins>/plugins/sse --force
cargo run -- sse https://example.com/events
cargo run -- install ws --path <path-to-netutils-plugins>/plugins/ws --force
cargo run -- ws wss://echo.websocket.events --message ping
cargo run -- install subdomain --path <path-to-netutils-plugins>/plugins/subdomain --force
cargo run -- subdomain example.com
```

See [PLUGIN_DEVELOPMENT.md](PLUGIN_DEVELOPMENT.md) for plugin layout, command conventions, testing, and release guidance.

## SDK

This workspace includes `netutils-plugin-sdk`, a small helper crate for plugin authors. It provides shared output primitives such as `OutputMode`, JSON printing, status text, color handling, and a small table renderer. Plugins still own their command-specific human output and JSON report schema.

## Plugins

| Plugin | Binary | Platforms | Description |
|--------|--------|-----------|-------------|
| `mcp` | `netutils-mcp` | windows, linux, macos | MCP Streamable HTTP diagnostics |
| `sse` | `netutils-sse` | windows, linux, macos | Server-Sent Events diagnostics |
| `subdomain` | `netutils-subdomain` | windows, linux, macos | Passive subdomain discovery from certificate transparency logs |
| `ws` | `netutils-ws` | windows, linux, macos | WebSocket diagnostics |

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

## SSE plugin

The SSE plugin connects to a `text/event-stream` endpoint and parses `event`, `id`, `retry`, and `data` fields.

Examples:

```bash
netutils sse https://example.com/events
netutils sse https://example.com/events -H "Authorization: Bearer xxx"
netutils sse https://example.com/events --max-events 10 --max-seconds 60
netutils sse https://example.com/events --proxy http://127.0.0.1:7897
```

## WebSocket plugin

The WebSocket plugin performs a WebSocket handshake, sends optional text messages, and receives the first messages.

Examples:

```bash
netutils ws wss://echo.websocket.events
netutils ws wss://echo.websocket.events --message ping
netutils ws https://example.com/socket -H "Authorization: Bearer xxx"
netutils websocket wss://echo.websocket.events --message ping
```

## Subdomain plugin

The Subdomain plugin discovers names below a domain from public certificate transparency logs. It is passive discovery, not a guaranteed full DNS zone dump.

Examples:

```bash
netutils subdomain example.com
netutils subdomain example.com --max 100
netutils subdomain example.com --include-wildcards
netutils --json subdomain example.com
```
