# netutils Plugin Development

This repository hosts external commands for `netutils-cli`. A plugin is a normal executable that follows a few naming, CLI, output, and release conventions so the core CLI can install and dispatch it predictably.

## Repository Layout

Use one workspace with one crate per plugin:

```text
netutils-plugins/
  Cargo.toml
  README.md
  PLUGIN_DEVELOPMENT.md
  crates/
    netutils-plugin-sdk/
      Cargo.toml
      src/
        lib.rs
  plugins/
    <plugin-name>/
      Cargo.toml
      plugin.toml
      src/
        main.rs
```

The workspace root `Cargo.toml` should include each plugin crate:

```toml
[workspace]
members = [
    "crates/netutils-plugin-sdk",
    "plugins/mcp",
    "plugins/sse",
    "plugins/ws",
]
resolver = "2"
```

## SDK

`netutils-plugin-sdk` is a lightweight helper crate for shared conventions. It does not move plugin rendering into the core CLI; it only gives plugins common building blocks:

- `OutputMode` for human vs JSON output
- `ColorMode` with `NO_COLOR` and `NETUTILS_COLOR=never` support
- `print_json`
- small table rendering
- status and error text helpers
- lightweight `PluginError` and `Result`

Use it from plugin crates:

```toml
[dependencies]
netutils-plugin-sdk = "0.1"
```

Local workspace plugins can use a path dependency while developing:

```toml
[dependencies]
netutils-plugin-sdk = { version = "0.1", path = "../../crates/netutils-plugin-sdk" }
```

## Naming

Keep names stable and mechanical:

| Item | Convention | Example |
|------|------------|---------|
| netutils command | short command name | `mcp` |
| binary | `netutils-<command>` | `netutils-mcp` |
| crate | `netutils-plugin-<command>` | `netutils-plugin-mcp` |
| plugin manifest | `plugins/<command>/plugin.toml` | `plugins/mcp/plugin.toml` |

The core CLI dispatches an unknown command such as `netutils mcp ...` to the installed binary `netutils-mcp`.

## Plugin Manifest

Each plugin should include `plugin.toml`:

```toml
name = "mcp"
binary = "netutils-mcp"
crate = "netutils-plugin-mcp"
description = "MCP Streamable HTTP diagnostics"
commands = ["mcp"]
```

Fields:

| Field | Meaning |
|-------|---------|
| `name` | canonical plugin name used by `netutils install <name>` |
| `binary` | executable name installed by Cargo |
| `crate` | crates.io package name |
| `description` | short user-facing description |
| `commands` | external command names provided by the plugin |

## CLI Contract

Plugins should behave like first-class `netutils` commands:

- Use `clap` or an equivalent parser.
- Support `--json` for machine-readable output.
- Support `--timeout <SECONDS>` for network operations.
- Support `--proxy <URL>` and `--no-proxy` when the command performs outbound HTTP/TCP requests.
- Print concise human-readable output by default.
- Return a non-zero exit code when the operation fails.
- Keep positional arguments and option names stable after release.

When the core CLI runs with `--json`, it forwards `--json` to the plugin. Plugin JSON should be a single JSON value printed to stdout.

The core also passes a small environment protocol to every plugin process:

| Variable | Values | Meaning |
|----------|--------|---------|
| `NETUTILS_OUTPUT` | `human`, `json` | Requested output mode |
| `NETUTILS_COLOR` | `auto`, `always`, `never` | Requested color mode |
| `NETUTILS_CORE_VERSION` | semver string | Core CLI version dispatching the plugin |
| `NETUTILS_PLUGIN_NAME` | command name | External command name used by the user |

Plugins should prefer explicit CLI flags when present, then fall back to these environment variables. `netutils-plugin-sdk` already does this for output and color mode.

## Output Guidelines

Human output should answer three questions:

- What endpoint or target was tested?
- Which phases succeeded or failed?
- What data should the user act on next?

JSON output should preserve raw protocol details when useful. For diagnostic commands, prefer this shape:

```json
{
  "target": "...",
  "summary": ["..."],
  "phases": {
    "connect": {
      "ok": true,
      "elapsed_ms": 12.3
    }
  }
}
```

Use stable field names. Add new fields instead of renaming existing fields.

## Installation

For released plugins, users install through the core CLI:

```bash
netutils install mcp
netutils mcp https://example.com/mcp
```

For local development, install from a checkout:

```bash
cd <path-to-netutils-cli>
cargo run -- install mcp --path <path-to-netutils-plugins>/plugins/mcp --force
cargo run -- mcp https://example.com/mcp
```

The core CLI may also auto-detect a sibling `netutils-plugins/plugins/<name>` checkout during development, but documentation and tests should use explicit `--path` when demonstrating local installs.

To create a new Rust plugin scaffold from the core CLI:

```bash
netutils plugin new whois
netutils plugin new whois --dir ./plugins --binary netutils-whois --crate netutils-plugin-whois
```

The scaffold includes `Cargo.toml`, `plugin.toml`, `README.md`, and `src/main.rs`. It uses `netutils-plugin-sdk` for basic output conventions.

## Testing

Each plugin crate should include unit tests for:

- argument parsing helpers
- protocol response parsing
- timeout and error classification helpers
- JSON output shape for important failure paths

Run all plugin tests from the workspace root:

```bash
cargo test
```

Before release, also test installation through the core CLI:

```bash
cd <path-to-netutils-cli>
cargo run -- install <plugin-name> --path <path-to-netutils-plugins>/plugins/<plugin-name> --force
cargo run -- <plugin-name> --help
cargo run -- plugin validate <path-to-netutils-plugins>/plugins/<plugin-name>
```

## Release Checklist

Before publishing a plugin:

1. Publish any required SDK version first, if the plugin depends on a new SDK release.
2. Update `plugins/<plugin-name>/Cargo.toml` version.
3. Update `README.md` examples if command behavior changed.
4. Run `cargo fmt`.
5. Run `cargo test`.
6. Run `cargo publish -p netutils-plugin-<plugin-name> --dry-run`.
7. Commit the changes.
8. Run `cargo publish -p netutils-plugin-<plugin-name>`.

Publish plugin crates before publishing a core CLI release that references them, so `netutils install <plugin-name>` can resolve the crate from crates.io.

## Compatibility

Plugins should be tolerant of older core dispatch behavior when practical, but should not silently accept ambiguous user input. A good pattern is to accept one known legacy shape and reject all other unexpected positional arguments with a clear message.

Avoid depending on private core internals. The stable integration surface is:

- the installed binary name
- forwarded CLI arguments
- `--json` propagation
- Cargo installation by crate name or local `--path`

## Current Plugins

| Plugin | Crate | Binary | Status |
|--------|-------|--------|--------|
| `mcp` | `netutils-plugin-mcp` | `netutils-mcp` | published |
