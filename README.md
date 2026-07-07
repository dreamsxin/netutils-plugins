# netutils plugins

Official external plugins for `netutils-cli`.

The core CLI discovers plugins as external subcommands. For example:

```bash
netutils install mcp
netutils mcp https://example.com/mcp
```

During local development from sibling checkouts:

```bash
cd D:\work\network-tools
cargo run -- install mcp --force
cargo run -- mcp https://example.com/mcp
```

## Plugins

| Plugin | Binary | Description |
|--------|--------|-------------|
| `mcp` | `netutils-mcp` | MCP Streamable HTTP diagnostics |
