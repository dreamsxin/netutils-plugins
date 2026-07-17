# netutils-plugin-sdk

Small helper SDK for external [`netutils-cli`](https://crates.io/crates/netutils-cli) plugins.

The SDK keeps plugin output conventions consistent without moving rendering into the core CLI. Plugins still own their command-specific human output and JSON schema.

## Features

- `OutputMode` for human vs JSON output
- `ColorMode` with `NO_COLOR` and `NETUTILS_COLOR` support
- `NETUTILS_OUTPUT=json` support
- JSON printing helpers
- small table renderer
- status text helpers
- lightweight `PluginError` and `Result`
- `proxy_for_url` support for explicit/core-forwarded/environment proxies, including `http/ws` and `https/wss` environment matching
- proxy URL and sensitive header redaction
- consistent non-zero failure exits

## Example

```rust
use netutils_plugin_sdk::{print_json, OutputMode};
use serde::Serialize;

#[derive(Serialize)]
struct Report {
    ok: bool,
}

let report = Report { ok: true };
print_json(&report);
```
