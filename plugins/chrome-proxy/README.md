# netutils chrome-proxy plugin

Launch Chrome through a local proxy bridge, then chain browser traffic to an upstream proxy.

The plugin starts a local HTTP proxy on `127.0.0.1:<port>`, launches Chrome with `--proxy-server`, and applies host resolver rules that block local DNS resolution for normal hostnames:

```text
MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost
```

This makes the test closer to a browser/TUN/proxy environment where the browser should send hostnames to the proxy path instead of resolving them locally.

## Install

```bash
netutils install chrome-proxy
```

## Usage

```bash
netutils chrome-proxy https://www.google.com/generate_204 --proxy socks5://127.0.0.1:7890
netutils chrome-proxy https://ipinfo.io --proxy http://user:pass@127.0.0.1:8080
netutils chrome-proxy https://www.youtube.com --proxy socks5h://127.0.0.1:7890 --show --wait 30
netutils --json chrome-proxy https://example.com --proxy socks5://127.0.0.1:7890
```

## Options

| Option | Description |
|--------|-------------|
| `--proxy <URL>` | Upstream proxy URL: `http`, `socks5`, or `socks5h` |
| `--listen <ADDR>` | Local bridge listen address, default `127.0.0.1:0` |
| `--chrome <PATH>` | Chrome executable path. Auto-detected when omitted |
| `--profile-dir <PATH>` | Chrome user data directory. A temporary profile is used when omitted |
| `--timeout <SECONDS>` | Chrome and bridge connection timeout, default `30` |
| `--wait <SECONDS>` | Visible Chrome observation window, default `20` |
| `--show` | Open visible Chrome instead of headless Chrome |
| `--keep-open` | With `--show`, keep the bridge alive until Chrome exits |
| `--json` | Print structured JSON output |

## Notes

- Supported upstream schemes: `http`, `socks5`, and `socks5h`.
- For SOCKS upstreams, the bridge sends domain names to the SOCKS server so remote DNS is used.
- The report is based on bridge traffic. A successful tunnel means Chrome reached the upstream proxy path for the requested host.
