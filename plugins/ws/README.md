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
netutils ws wss://echo.websocket.events --proxy http://127.0.0.1:7890
netutils ws wss://echo.websocket.events --proxy socks5h://127.0.0.1:1080
netutils ws wss://echo.websocket.events --no-proxy
```

`--proxy` supports HTTP CONNECT and SOCKS5/SOCKS5H tunnels. The core may also
forward the target-specific system proxy through `NETUTILS_EFFECTIVE_PROXY`;
`--no-proxy` disables both forwarded and environment proxy selection.

Use `--json` for machine-readable output.
