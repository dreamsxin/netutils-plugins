# netutils-ws

WebSocket handshake and message diagnostics plugin for `netutils-cli`.

## Install

```bash
netutils install ws
```

## Usage

```bash
netutils ws wss://echo.websocket.events
netutils ws wss://echo.websocket.events --message ping
netutils ws wss://echo.websocket.events -H "Authorization: Bearer xxx"
```

Use `--json` for machine-readable output.
