# netutils-sse

Server-Sent Events diagnostics plugin for `netutils-cli`.

## Install

```bash
netutils install sse
```

## Usage

```bash
netutils sse https://example.com/events
netutils sse https://example.com/events -H "Authorization: Bearer xxx"
netutils sse https://example.com/events --max-events 10 --max-seconds 60
netutils sse https://example.com/events --proxy http://127.0.0.1:7897
```

Use `--json` for machine-readable output.
