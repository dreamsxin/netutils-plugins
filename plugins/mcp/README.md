# netutils-plugin-mcp

MCP Streamable HTTP diagnostics plugin for [`netutils-cli`](https://crates.io/crates/netutils-cli).

This plugin provides the external `netutils mcp ...` command through the `netutils-mcp` binary. It tests MCP Streamable HTTP endpoints by running the standard initialization flow and, optionally, a tool call.

## Install

Install the core CLI first:

```bash
cargo install netutils-cli
```

Then install the MCP plugin through `netutils`:

```bash
netutils install mcp
```

You can also install the plugin binary directly:

```bash
cargo install netutils-plugin-mcp
```

## Usage

List server tools:

```bash
netutils mcp https://example.com/mcp
```

Add request headers:

```bash
netutils mcp https://example.com/mcp -H "Authorization: Bearer xxx"
```

Use a specific MCP protocol version:

```bash
netutils mcp https://example.com/mcp --protocol-version 2025-11-25
```

Listen to the optional server-to-client SSE stream after initialization:

```bash
netutils mcp https://example.com/mcp --listen --max-events 5 --max-seconds 30
```

Call one MCP tool:

```bash
netutils mcp https://example.com/mcp --tool search --args '{"query":"netutils"}'
```

Require the tool to appear in `tools/list` before calling it:

```bash
netutils mcp https://example.com/mcp --tool search --args '{"query":"netutils"}' --require-tool
```

Force direct access and ignore proxies:

```bash
netutils mcp https://example.com/mcp --no-proxy
```

Use an explicit proxy:

```bash
netutils mcp https://example.com/mcp --proxy socks5h://127.0.0.1:7890
```

Emit JSON:

```bash
netutils --json mcp https://example.com/mcp --tool search --args '{"query":"netutils"}'
```

## What It Checks

The plugin performs:

- `initialize`
- `notifications/initialized`
- `tools/list` by default
- optional `tools/call`
- optional GET server-to-client SSE listen stream

It accepts MCP JSON-RPC responses returned as either `application/json` or `text/event-stream`.

## Options

| Option | Description |
|--------|-------------|
| `--json` | Print a single JSON report |
| `-H, --header <HEADER>` | Add a request header, repeatable |
| `--timeout <SECONDS>` | Request/connect timeout |
| `--protocol-version <VERSION>` | MCP protocol version sent during initialization |
| `--no-tools` | Skip `tools/list` |
| `--tool <NAME>` | Call one MCP tool with `tools/call` |
| `--args <JSON>` | JSON object passed as `params.arguments`; defaults to `{}` |
| `--require-tool` | Only call the tool if it appears in `tools/list` |
| `--listen` | Open a GET SSE stream after initialization |
| `--max-events <N>` | Max SSE events to collect in listen mode |
| `--max-seconds <SECONDS>` | Max seconds to listen |
| `--proxy <URL>` | Use an HTTP/SOCKS proxy |
| `--no-proxy` | Force direct access and ignore proxies |

## Output

Human output summarizes each phase and prints discovered tools or the tool call result.

JSON output includes raw protocol details. When `--tool` is used, the raw `tools/call` response is available under `tool_call`.

## Repository

Source code: <https://github.com/dreamsxin/netutils-plugins>
