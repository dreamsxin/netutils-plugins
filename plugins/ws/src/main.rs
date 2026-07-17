// WebSocket handshake and message diagnostics.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use base64::Engine;
use clap::Parser;
use colored::*;
use futures_util::{SinkExt, StreamExt};
use netutils_plugin_sdk::{
    exit_on_failure, print_json, print_table, proxy_for_url, redact_url_credentials, OutputMode,
};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::client_async_tls_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::{Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

#[derive(Parser, Debug)]
#[command(
    name = "netutils-ws",
    version,
    about = "WebSocket handshake and message diagnostics"
)]
struct Cli {
    /// JSON output
    #[arg(long)]
    json: bool,

    /// WebSocket URL, defaults to wss:// when scheme is omitted; http(s) converts to ws(s)
    url: String,

    /// Request header, repeatable, for example -H "Authorization: Bearer xxx"
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Connect/read timeout seconds
    #[arg(long, default_value_t = 10)]
    timeout: u64,

    /// Proxy URL, for example http://127.0.0.1:7890 or socks5h://127.0.0.1:1080
    #[arg(long)]
    proxy: Option<String>,

    /// Disable explicit, core-forwarded, and environment proxy selection
    #[arg(long)]
    no_proxy: bool,

    /// Text message to send after connecting, repeatable
    #[arg(long = "message")]
    messages: Vec<String>,

    /// Max messages to receive
    #[arg(long, default_value_t = 5)]
    max_messages: usize,

    /// Max seconds to listen
    #[arg(long, default_value_t = 30)]
    max_seconds: u64,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mode = OutputMode::from_json_flag(cli.json);
    run(
        &cli.url,
        cli.headers,
        Duration::from_secs(cli.timeout),
        cli.proxy,
        cli.no_proxy,
        cli.messages,
        cli.max_messages,
        Duration::from_secs(cli.max_seconds),
        mode,
    )
    .await;
}

#[derive(Debug, Serialize)]
pub struct WsReport {
    pub input_url: String,
    pub url: String,
    pub proxy: WsProxy,
    pub connected: bool,
    pub status: Option<u16>,
    pub sent: Vec<WsMessage>,
    pub received: Vec<WsMessage>,
    pub error: Option<String>,
    pub elapsed_ms: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsProxy {
    pub mode: String,
    pub value: Option<String>,
    pub tunnel: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsMessage {
    pub index: usize,
    pub kind: String,
    pub bytes: usize,
    pub text: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    input_url: &str,
    raw_headers: Vec<String>,
    timeout: Duration,
    proxy: Option<String>,
    no_proxy: bool,
    messages: Vec<String>,
    max_messages: usize,
    max_duration: Duration,
    mode: OutputMode,
) {
    let start = Instant::now();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = normalize_url(input_url);
    let effective_proxy = proxy_for_url(&url, proxy, no_proxy);
    let proxy_report = WsProxy {
        mode: if no_proxy {
            "direct-forced".to_string()
        } else if effective_proxy.is_some() {
            "proxy".to_string()
        } else {
            "direct".to_string()
        },
        value: effective_proxy.as_deref().map(redact_url_credentials),
        tunnel: effective_proxy
            .as_deref()
            .map(proxy_tunnel_label)
            .unwrap_or("none")
            .to_string(),
    };
    let mut request = match url.as_str().into_client_request() {
        Ok(request) => request,
        Err(err) => {
            output(
                error_report(
                    input_url,
                    &url,
                    proxy_report,
                    format!("invalid WebSocket URL: {err}"),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
    };
    if let Err(err) = apply_headers(request.headers_mut(), &raw_headers) {
        output(
            error_report(input_url, &url, proxy_report, err, start.elapsed()),
            mode,
        );
        return;
    }

    let (mut stream, response) = match tokio::time::timeout(
        timeout,
        connect_with_optional_proxy(request, effective_proxy.as_deref()),
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => {
            output(
                error_report(input_url, &url, proxy_report, err, start.elapsed()),
                mode,
            );
            return;
        }
        Err(_) => {
            output(
                error_report(
                    input_url,
                    &url,
                    proxy_report,
                    "connect timeout".to_string(),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
    };

    let mut sent = Vec::new();
    for (idx, message) in messages.into_iter().enumerate() {
        let bytes = message.len();
        match tokio::time::timeout(timeout, stream.send(Message::Text(message.clone().into())))
            .await
        {
            Ok(Ok(())) => sent.push(WsMessage {
                index: idx + 1,
                kind: "text".to_string(),
                bytes,
                text: Some(message),
            }),
            Ok(Err(err)) => {
                output(
                    report(
                        input_url,
                        &url,
                        proxy_report,
                        true,
                        Some(response.status().as_u16()),
                        sent,
                        Vec::new(),
                        Some(format!("send failed: {err}")),
                        start.elapsed(),
                    ),
                    mode,
                );
                return;
            }
            Err(_) => {
                output(
                    report(
                        input_url,
                        &url,
                        proxy_report,
                        true,
                        Some(response.status().as_u16()),
                        sent,
                        Vec::new(),
                        Some("send timeout".to_string()),
                        start.elapsed(),
                    ),
                    mode,
                );
                return;
            }
        }
    }

    let deadline = tokio::time::Instant::now() + max_duration;
    let mut received = Vec::new();
    let mut error = None;
    while received.len() < max_messages && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(message))) => {
                let converted = convert_message(received.len() + 1, message);
                let is_close = converted.kind == "close";
                received.push(converted);
                if is_close {
                    break;
                }
            }
            Ok(Some(Err(err))) => {
                error = Some(err.to_string());
                break;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    output(
        report(
            input_url,
            &url,
            proxy_report,
            true,
            Some(response.status().as_u16()),
            sent,
            received,
            error,
            start.elapsed(),
        ),
        mode,
    );
}

fn normalize_url(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = input.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if input.starts_with("ws://") || input.starts_with("wss://") {
        input.to_string()
    } else {
        format!("wss://{input}")
    }
}

fn apply_headers(
    headers: &mut tokio_tungstenite::tungstenite::http::HeaderMap,
    raw_headers: &[String],
) -> Result<(), String> {
    for raw in raw_headers {
        let Some((name, value)) = raw.split_once(':') else {
            return Err(format!("invalid header, expected 'Name: value': {raw}"));
        };
        let name = HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|err| format!("invalid header name '{name}': {err}"))?;
        let value = HeaderValue::from_str(value.trim())
            .map_err(|err| format!("invalid header value for '{name}': {err}"))?;
        headers.append(name, value);
    }
    Ok(())
}

type WsConnectResult = Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), String>;

#[derive(Debug)]
struct TargetEndpoint {
    host: String,
    port: u16,
}

#[derive(Debug)]
struct ProxyEndpoint {
    scheme: String,
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
}

async fn connect_with_optional_proxy(request: Request, proxy: Option<&str>) -> WsConnectResult {
    let target = target_from_request(&request)?;
    let stream = match proxy {
        Some(proxy) => connect_via_proxy(proxy, &target).await?,
        None => TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|err| format!("TCP connect failed: {err}"))?,
    };
    client_async_tls_with_config(request, stream, None, None)
        .await
        .map_err(|err| err.to_string())
}

fn target_from_request(request: &Request) -> Result<TargetEndpoint, String> {
    let uri = request.uri();
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| "missing WebSocket URL scheme".to_string())?
        .to_ascii_lowercase();
    if scheme != "ws" && scheme != "wss" {
        return Err(format!("unsupported WebSocket URL scheme: {scheme}"));
    }
    let host = uri
        .host()
        .ok_or_else(|| "missing WebSocket URL host".to_string())?
        .to_string();
    let port = uri
        .port_u16()
        .unwrap_or(if scheme == "wss" { 443 } else { 80 });
    Ok(TargetEndpoint { host, port })
}

async fn connect_via_proxy(proxy: &str, target: &TargetEndpoint) -> Result<TcpStream, String> {
    let proxy = parse_proxy_endpoint(proxy)?;
    let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|err| format!("proxy TCP connect failed: {err}"))?;
    match proxy.scheme.as_str() {
        "http" => {
            http_connect(&mut stream, &proxy, target).await?;
            Ok(stream)
        }
        "socks" | "socks5" | "socks5h" => {
            socks5_connect(&mut stream, &proxy, target).await?;
            Ok(stream)
        }
        "https" => Err("HTTPS proxy tunneling is not supported by this plugin yet".to_string()),
        "socks4" | "socks4a" => {
            Err("SOCKS4 proxy tunneling is not supported by this plugin yet".to_string())
        }
        other => Err(format!("unsupported proxy scheme: {other}")),
    }
}

async fn http_connect(
    stream: &mut TcpStream,
    proxy: &ProxyEndpoint,
    target: &TargetEndpoint,
) -> Result<(), String> {
    let authority = host_port(&target.host, target.port);
    let mut request =
        format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: netutils-ws\r\n");
    if let Some(auth) = proxy_basic_auth(proxy) {
        request.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("proxy CONNECT write failed: {err}"))?;

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    while response.len() < 16 * 1024 {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|err| format!("proxy CONNECT read failed: {err}"))?;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buf[..n]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&response);
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| "proxy CONNECT returned an invalid response".to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("proxy CONNECT failed with status {status}"));
    }
    Ok(())
}

async fn socks5_connect(
    stream: &mut TcpStream,
    proxy: &ProxyEndpoint,
    target: &TargetEndpoint,
) -> Result<(), String> {
    let has_auth = !proxy.username.is_empty();
    let methods = if has_auth {
        vec![0x05, 0x02, 0x00, 0x02]
    } else {
        vec![0x05, 0x01, 0x00]
    };
    stream
        .write_all(&methods)
        .await
        .map_err(|err| format!("SOCKS5 greeting write failed: {err}"))?;
    let mut response = [0u8; 2];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|err| format!("SOCKS5 greeting read failed: {err}"))?;
    if response[0] != 0x05 {
        return Err("SOCKS5 proxy returned an invalid version".to_string());
    }
    match response[1] {
        0x00 => {}
        0x02 => socks5_auth(stream, proxy).await?,
        0xff => return Err("SOCKS5 proxy rejected all authentication methods".to_string()),
        method => {
            return Err(format!(
                "SOCKS5 proxy selected unsupported auth method {method}"
            ))
        }
    }

    let mut request = vec![0x05, 0x01, 0x00];
    append_socks5_addr(&mut request, &target.host)?;
    request.extend_from_slice(&target.port.to_be_bytes());
    stream
        .write_all(&request)
        .await
        .map_err(|err| format!("SOCKS5 CONNECT write failed: {err}"))?;

    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|err| format!("SOCKS5 CONNECT read failed: {err}"))?;
    if header[0] != 0x05 {
        return Err("SOCKS5 CONNECT returned an invalid version".to_string());
    }
    if header[1] != 0x00 {
        return Err(format!(
            "SOCKS5 CONNECT failed: {}",
            socks5_reply_text(header[1])
        ));
    }
    read_socks5_bound_addr(stream, header[3]).await?;
    Ok(())
}

async fn socks5_auth(stream: &mut TcpStream, proxy: &ProxyEndpoint) -> Result<(), String> {
    let username = proxy.username.as_bytes();
    let password = proxy.password.as_deref().unwrap_or("").as_bytes();
    if username.len() > u8::MAX as usize || password.len() > u8::MAX as usize {
        return Err("SOCKS5 username/password is too long".to_string());
    }
    let mut request = vec![0x01, username.len() as u8];
    request.extend_from_slice(username);
    request.push(password.len() as u8);
    request.extend_from_slice(password);
    stream
        .write_all(&request)
        .await
        .map_err(|err| format!("SOCKS5 auth write failed: {err}"))?;
    let mut response = [0u8; 2];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|err| format!("SOCKS5 auth read failed: {err}"))?;
    if response != [0x01, 0x00] {
        return Err("SOCKS5 authentication failed".to_string());
    }
    Ok(())
}

fn append_socks5_addr(request: &mut Vec<u8>, host: &str) -> Result<(), String> {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            request.push(0x04);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let host = host.as_bytes();
            if host.len() > u8::MAX as usize {
                return Err("SOCKS5 target host is too long".to_string());
            }
            request.push(0x03);
            request.push(host.len() as u8);
            request.extend_from_slice(host);
        }
    }
    Ok(())
}

async fn read_socks5_bound_addr(stream: &mut TcpStream, atyp: u8) -> Result<(), String> {
    let len = match atyp {
        0x01 => 4,
        0x03 => {
            let mut len = [0u8; 1];
            stream
                .read_exact(&mut len)
                .await
                .map_err(|err| format!("SOCKS5 bound address read failed: {err}"))?;
            len[0] as usize
        }
        0x04 => 16,
        other => {
            return Err(format!(
                "SOCKS5 CONNECT returned unsupported address type {other}"
            ))
        }
    };
    let mut rest = vec![0u8; len + 2];
    stream
        .read_exact(&mut rest)
        .await
        .map_err(|err| format!("SOCKS5 bound address read failed: {err}"))?;
    Ok(())
}

fn parse_proxy_endpoint(proxy: &str) -> Result<ProxyEndpoint, String> {
    let value = if proxy.contains("://") {
        proxy.to_string()
    } else {
        format!("http://{proxy}")
    };
    let parsed = url::Url::parse(&value).map_err(|err| format!("invalid proxy URL: {err}"))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed
        .host_str()
        .ok_or_else(|| "proxy URL is missing host".to_string())?
        .to_string();
    let default_port = match scheme.as_str() {
        "https" => 443,
        "socks" | "socks4" | "socks4a" | "socks5" | "socks5h" => 1080,
        _ => 80,
    };
    let port = parsed.port().unwrap_or(default_port);
    Ok(ProxyEndpoint {
        scheme,
        host,
        port,
        username: parsed.username().to_string(),
        password: parsed.password().map(ToString::to_string),
    })
}

fn proxy_basic_auth(proxy: &ProxyEndpoint) -> Option<String> {
    if proxy.username.is_empty() {
        return None;
    }
    let password = proxy.password.as_deref().unwrap_or("");
    Some(
        base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", proxy.username, password)),
    )
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn proxy_tunnel_label(proxy: &str) -> &'static str {
    let scheme = proxy
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .unwrap_or_else(|| "http".to_string());
    match scheme.as_str() {
        "http" => "http-connect",
        "socks" | "socks5" | "socks5h" => "socks5",
        "https" => "https-proxy-unsupported",
        "socks4" | "socks4a" => "socks4-unsupported",
        _ => "unsupported",
    }
}

fn socks5_reply_text(code: u8) -> &'static str {
    match code {
        0x01 => "general failure",
        0x02 => "connection not allowed",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown error",
    }
}

fn convert_message(index: usize, message: Message) -> WsMessage {
    match message {
        Message::Text(text) => {
            let text = text.to_string();
            WsMessage {
                index,
                kind: "text".to_string(),
                bytes: text.len(),
                text: Some(text),
            }
        }
        Message::Binary(bytes) => WsMessage {
            index,
            kind: "binary".to_string(),
            bytes: bytes.len(),
            text: None,
        },
        Message::Ping(bytes) => WsMessage {
            index,
            kind: "ping".to_string(),
            bytes: bytes.len(),
            text: None,
        },
        Message::Pong(bytes) => WsMessage {
            index,
            kind: "pong".to_string(),
            bytes: bytes.len(),
            text: None,
        },
        Message::Close(frame) => WsMessage {
            index,
            kind: "close".to_string(),
            bytes: 0,
            text: frame.map(|frame| format!("{:?} {}", frame.code, frame.reason)),
        },
        Message::Frame(_) => WsMessage {
            index,
            kind: "frame".to_string(),
            bytes: 0,
            text: None,
        },
    }
}

fn error_report(
    input_url: &str,
    url: &str,
    proxy: WsProxy,
    error: String,
    elapsed: Duration,
) -> WsReport {
    report(
        input_url,
        url,
        proxy,
        false,
        None,
        Vec::new(),
        Vec::new(),
        Some(error),
        elapsed,
    )
}

#[allow(clippy::too_many_arguments)]
fn report(
    input_url: &str,
    url: &str,
    proxy: WsProxy,
    connected: bool,
    status: Option<u16>,
    sent: Vec<WsMessage>,
    received: Vec<WsMessage>,
    error: Option<String>,
    elapsed: Duration,
) -> WsReport {
    WsReport {
        input_url: input_url.to_string(),
        url: url.to_string(),
        proxy,
        connected,
        status,
        sent,
        received,
        error,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        notes: vec![
            "Use --message repeatedly to send text messages after the WebSocket handshake."
                .to_string(),
            "Proxy mode uses HTTP CONNECT or SOCKS5 tunneling before the WebSocket handshake."
                .to_string(),
        ],
    }
}

fn output(report: WsReport, mode: OutputMode) {
    let failed = !report.connected || report.error.is_some();
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
    exit_on_failure(failed);
}

fn print_report(report: &WsReport) {
    println!();
    println!("{}", "🔌 WebSocket Probe".bold());
    println!("  URL: {}", report.url);
    println!("  Proxy: {}", report.proxy.mode);
    if let Some(proxy) = &report.proxy.value {
        println!("  Proxy Value: {}", proxy);
    }
    println!("  Tunnel: {}", report.proxy.tunnel);
    println!(
        "  Connected: {}",
        if report.connected {
            "yes".green().to_string()
        } else {
            "no".red().to_string()
        }
    );
    println!(
        "  Status: {}",
        report
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    println!("  Time: {:.2}ms", report.elapsed_ms);
    if let Some(error) = &report.error {
        println!("  Error: {}", error);
    }

    if !report.sent.is_empty() {
        println!();
        println!("{}", "Sent".bold());
        print_messages(&report.sent);
    }
    if !report.received.is_empty() {
        println!();
        println!("{}", "Received".bold());
        print_messages(&report.received);
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn print_messages(messages: &[WsMessage]) {
    let rows = messages
        .iter()
        .map(|message| {
            vec![
                message.index.to_string(),
                message.kind.clone(),
                message.bytes.to_string(),
                message
                    .text
                    .as_ref()
                    .map(|text| truncate(text, 120))
                    .unwrap_or_else(|| "--".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    print_table(&["#", "Type", "Bytes", "Text"], &rows);
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push_str("...");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_websocket_urls() {
        assert_eq!(normalize_url("example.com/ws"), "wss://example.com/ws");
        assert_eq!(
            normalize_url("https://example.com/ws"),
            "wss://example.com/ws"
        );
        assert_eq!(
            normalize_url("http://example.com/ws"),
            "ws://example.com/ws"
        );
        assert_eq!(normalize_url("ws://example.com/ws"), "ws://example.com/ws");
    }

    #[test]
    fn rejects_invalid_header() {
        let mut headers = tokio_tungstenite::tungstenite::http::HeaderMap::new();
        assert!(apply_headers(&mut headers, &["bad".to_string()]).is_err());
    }

    #[test]
    fn parses_proxy_endpoints() {
        let proxy = parse_proxy_endpoint("127.0.0.1:7890").unwrap();
        assert_eq!(proxy.scheme, "http");
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 7890);

        let proxy = parse_proxy_endpoint("socks5h://user:pass@example.com").unwrap();
        assert_eq!(proxy.scheme, "socks5h");
        assert_eq!(proxy.host, "example.com");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.username, "user");
        assert_eq!(proxy.password.as_deref(), Some("pass"));
    }

    #[test]
    fn labels_proxy_tunnels() {
        assert_eq!(proxy_tunnel_label("http://127.0.0.1:7890"), "http-connect");
        assert_eq!(proxy_tunnel_label("socks5h://127.0.0.1:1080"), "socks5");
        assert_eq!(
            proxy_tunnel_label("https://127.0.0.1:8443"),
            "https-proxy-unsupported"
        );
    }
}
