# netutils subdomain plugin

Discover subdomains for a domain from public certificate transparency logs.

This plugin is a passive discovery tool. It does not guarantee every subdomain exists, because DNS has no standard query for "all subdomains". Internal names, private zones, names without public certificates, and names hidden behind private DNS will not appear.

## Install

```bash
netutils install subdomain
```

## Usage

```bash
netutils subdomain example.com
netutils subdomain example.com --max 100
netutils subdomain example.com --include-wildcards
netutils --json subdomain example.com
```

## Options

| Option | Description |
|--------|-------------|
| `--source crtsh\|bufferover\|all` | Discovery source, default `all` |
| `--timeout <SECONDS>` | HTTP request timeout, default `15` |
| `--max <N>` | Maximum rows to print/return, default `5000` |
| `--include-wildcards` | Keep wildcard names such as `*.api.example.com`; otherwise the wildcard prefix is stripped |
| `--json` | Print structured JSON output |

## Notes

- The plugin queries public certificate transparency and passive DNS sources.
- Results are normalized to lowercase, deduplicated, and limited to names below the requested domain.
- Certificate transparency data can contain stale records for names that no longer resolve.
