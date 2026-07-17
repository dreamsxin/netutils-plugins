//! Chrome proxy-chain diagnostics through a local bridge.

use std::{
    env, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use clap::Parser;
use netutils_plugin_sdk::{print_json, print_table, OutputMode};
use serde::Serialize;
use tempfile::TempDir;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::mpsc,
};
use tokio_socks::tcp::Socks5Stream;
use url::Url;

const RESOLVER_RULES: &str = "MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost";

#[derive(Parser, Debug)]
#[command(
    name = "netutils-chrome-proxy",
    version,
    about = "Launch Chrome through a local chained proxy bridge"
)]
struct Cli {
    /// JSON output
    #[arg(long)]
    json: bool,

    /// URL Chrome should visit
    #[arg(default_value = "https://www.google.com/generate_204")]
    url: String,

    /// Upstream proxy URL: http://, socks5://, or socks5h://
    #[arg(long)]
    proxy: String,

    /// Local bridge listen address
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: String,

    /// Chrome executable path
    #[arg(long)]
    chrome: Option<PathBuf>,

    /// Chrome profile directory. A temporary profile is used when omitted
    #[arg(long)]
    profile_dir: Option<PathBuf>,

    /// Chrome and bridge connection timeout seconds
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Visible Chrome observation window seconds
    #[arg(long, default_value_t = 20)]
    wait: u64,

    /// Open visible Chrome instead of headless Chrome
    #[arg(long)]
    show: bool,

    /// With --show, keep the bridge alive until Chrome exits
    #[arg(long)]
    keep_open: bool,
}

#[derive(Debug, Serialize)]
struct ChromeProxyReport {
    target_url: String,
    upstream_proxy: String,
    local_proxy: Option<String>,
    chrome_path: Option<String>,
    profile_dir: Option<String>,
    mode: String,
    resolver_rules: String,
    launched: bool,
    exit_status: Option<String>,
    timed_out: bool,
    observed_requests: usize,
    successful_tunnels: usize,
    requests: Vec<BridgeEvent>,
    error: Option<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BridgeEvent {
    method: String,
    target: String,
    upstream: String,
    result: String,
    elapsed_ms: f64,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct UpstreamProxy {
    scheme: ProxyScheme,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    redacted: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProxyScheme {
    Http,
    Socks5,
}

trait ProxyIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> ProxyIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mode = OutputMode::from_json_flag(cli.json);
    let report = run(cli).await;
    let failed = report.error.is_some() || report.successful_tunnels == 0;
    output(&report, mode);
    if failed {
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> ChromeProxyReport {
    let upstream = match parse_upstream_proxy(&cli.proxy) {
        Ok(proxy) => Arc::new(proxy),
        Err(err) => return error_report(&cli, None, None, None, err),
    };
    let chrome_path = match cli.chrome.clone().or_else(find_chrome) {
        Some(path) => path,
        None => {
            return error_report(
                &cli,
                None,
                None,
                Some(upstream.redacted.clone()),
                "Chrome executable was not found; pass --chrome <PATH>".to_string(),
            )
        }
    };

    let listener = match TcpListener::bind(&cli.listen).await {
        Ok(listener) => listener,
        Err(err) => {
            return error_report(
                &cli,
                None,
                Some(chrome_path),
                Some(upstream.redacted.clone()),
                format!("failed to bind local bridge: {err}"),
            )
        }
    };
    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(err) => {
            return error_report(
                &cli,
                None,
                Some(chrome_path),
                Some(upstream.redacted.clone()),
                format!("failed to read local bridge address: {err}"),
            )
        }
    };

    let (tx, mut rx) = mpsc::channel(128);
    tokio::spawn(accept_loop(
        listener,
        upstream.clone(),
        tx,
        Duration::from_secs(cli.timeout),
    ));

    let temp_profile = if cli.profile_dir.is_none() {
        match tempfile::Builder::new()
            .prefix("netutils-chrome-proxy-")
            .tempdir()
        {
            Ok(dir) => Some(dir),
            Err(err) => {
                return error_report(
                    &cli,
                    Some(local_addr),
                    Some(chrome_path),
                    Some(upstream.redacted.clone()),
                    format!("failed to create temporary Chrome profile: {err}"),
                )
            }
        }
    } else {
        None
    };
    let profile_dir = cli.profile_dir.clone().or_else(|| {
        temp_profile
            .as_ref()
            .map(TempDir::path)
            .map(Path::to_path_buf)
    });

    let chrome_result = launch_chrome(
        &cli,
        &chrome_path,
        profile_dir.as_deref(),
        local_addr,
        Duration::from_secs(cli.timeout),
    )
    .await;

    let mut requests = Vec::new();
    drain_events(&mut rx, &mut requests);

    let (launched, exit_status, timed_out, error) = match chrome_result {
        Ok(result) => (true, result.exit_status, result.timed_out, None),
        Err(err) => (false, None, false, Some(err)),
    };
    let successful_tunnels = requests.iter().filter(|event| event.result == "ok").count();
    let error = error.or_else(|| {
        (successful_tunnels == 0).then(|| {
            "no successful proxy tunnel was observed from Chrome through the local bridge"
                .to_string()
        })
    });

    ChromeProxyReport {
        target_url: cli.url,
        upstream_proxy: upstream.redacted.clone(),
        local_proxy: Some(format!("http://{local_addr}")),
        chrome_path: Some(chrome_path.display().to_string()),
        profile_dir: profile_dir.map(|path| path.display().to_string()),
        mode: if cli.show {
            "visible".to_string()
        } else {
            "headless".to_string()
        },
        resolver_rules: RESOLVER_RULES.to_string(),
        launched,
        exit_status,
        timed_out,
        observed_requests: requests.len(),
        successful_tunnels,
        requests,
        error,
        notes: report_notes(),
    }
}

#[derive(Debug)]
struct ChromeRunResult {
    exit_status: Option<String>,
    timed_out: bool,
}

async fn launch_chrome(
    cli: &Cli,
    chrome_path: &Path,
    profile_dir: Option<&Path>,
    local_addr: SocketAddr,
    timeout: Duration,
) -> Result<ChromeRunResult, String> {
    let mut command = Command::new(chrome_path);
    command
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg(format!("--proxy-server=http://{local_addr}"))
        .arg(format!("--host-resolver-rules={RESOLVER_RULES}"));
    if let Some(profile_dir) = profile_dir {
        command.arg(format!("--user-data-dir={}", profile_dir.display()));
    }
    if cli.show {
        command.arg("--new-window").arg(&cli.url);
        command.stdin(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|err| format!("failed to launch Chrome: {err}"))?;
        if cli.keep_open {
            let status = child
                .wait()
                .await
                .map_err(|err| format!("failed to wait for Chrome: {err}"))?;
            return Ok(ChromeRunResult {
                exit_status: Some(status.to_string()),
                timed_out: false,
            });
        }
        tokio::time::sleep(Duration::from_secs(cli.wait)).await;
        let status = match child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => {
                let _ = child.kill().await;
                Some("killed after observation window".to_string())
            }
            Err(err) => Some(format!("failed to read Chrome status: {err}")),
        };
        return Ok(ChromeRunResult {
            exit_status: status,
            timed_out: false,
        });
    }

    command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--dump-dom")
        .arg(&cli.url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to launch Chrome: {err}"))?;
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(ChromeRunResult {
            exit_status: Some(status.to_string()),
            timed_out: false,
        }),
        Ok(Err(err)) => Err(format!("failed to wait for Chrome: {err}")),
        Err(_) => {
            let _ = child.kill().await;
            Ok(ChromeRunResult {
                exit_status: Some("killed after timeout".to_string()),
                timed_out: true,
            })
        }
    }
}

async fn accept_loop(
    listener: TcpListener,
    upstream: Arc<UpstreamProxy>,
    tx: mpsc::Sender<BridgeEvent>,
    timeout: Duration,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let upstream = upstream.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            handle_client(stream, upstream, tx, timeout).await;
        });
    }
}

async fn handle_client(
    mut browser: TcpStream,
    upstream: Arc<UpstreamProxy>,
    tx: mpsc::Sender<BridgeEvent>,
    timeout: Duration,
) {
    let start = Instant::now();
    let head = match tokio::time::timeout(timeout, read_http_head(&mut browser)).await {
        Ok(Ok(head)) => head,
        Ok(Err(err)) => {
            let _ = send_event(
                &tx,
                "UNKNOWN",
                "--",
                &upstream.redacted,
                "failed",
                start,
                Some(format!("failed to read browser request: {err}")),
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = send_event(
                &tx,
                "UNKNOWN",
                "--",
                &upstream.redacted,
                "failed",
                start,
                Some("timed out reading browser request".to_string()),
            )
            .await;
            return;
        }
    };
    let request = match parse_browser_request(&head) {
        Ok(request) => request,
        Err(err) => {
            let _ = send_event(
                &tx,
                "UNKNOWN",
                "--",
                &upstream.redacted,
                "failed",
                start,
                Some(err),
            )
            .await;
            return;
        }
    };

    let mut upstream_stream = match connect_via_upstream(&upstream, &request.target, timeout).await
    {
        Ok(stream) => stream,
        Err(err) => {
            let _ = browser
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = send_event(
                &tx,
                &request.method,
                &request.target.label(),
                &upstream.redacted,
                "failed",
                start,
                Some(err),
            )
            .await;
            return;
        }
    };

    if request.method.eq_ignore_ascii_case("CONNECT") {
        if browser
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .is_err()
        {
            return;
        }
    } else if upstream_stream
        .write_all(&rewrite_http_request(&head, &request))
        .await
        .is_err()
    {
        let _ = send_event(
            &tx,
            &request.method,
            &request.target.label(),
            &upstream.redacted,
            "failed",
            start,
            Some("failed to forward HTTP request to upstream tunnel".to_string()),
        )
        .await;
        return;
    }

    let _ = send_event(
        &tx,
        &request.method,
        &request.target.label(),
        &upstream.redacted,
        "ok",
        start,
        None,
    )
    .await;
    let _ = tokio::io::copy_bidirectional(&mut browser, upstream_stream.as_mut()).await;
}

async fn read_http_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header is larger than 64KiB",
            ));
        }
    }
    Ok(buf)
}

#[derive(Debug)]
struct BrowserRequest {
    method: String,
    target: TargetAddr,
    path: String,
}

#[derive(Debug)]
struct TargetAddr {
    host: String,
    port: u16,
}

impl TargetAddr {
    fn label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn parse_browser_request(head: &[u8]) -> Result<BrowserRequest, String> {
    let text = String::from_utf8_lossy(head);
    let first_line = text
        .lines()
        .next()
        .ok_or_else(|| "empty browser request".to_string())?;
    let mut parts = first_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "missing HTTP method".to_string())?
        .to_string();
    let uri = parts
        .next()
        .ok_or_else(|| "missing HTTP request target".to_string())?;

    if method.eq_ignore_ascii_case("CONNECT") {
        return Ok(BrowserRequest {
            method,
            target: parse_host_port(uri, 443)?,
            path: String::new(),
        });
    }

    if let Ok(url) = Url::parse(uri) {
        let host = url
            .host_str()
            .ok_or_else(|| format!("HTTP URL has no host: {uri}"))?
            .to_string();
        let port = url.port_or_known_default().unwrap_or(80);
        let mut path = url.path().to_string();
        if path.is_empty() {
            path.push('/');
        }
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }
        return Ok(BrowserRequest {
            method,
            target: TargetAddr { host, port },
            path,
        });
    }

    let host =
        header_value(&text, "host").ok_or_else(|| "HTTP request has no Host header".to_string())?;
    Ok(BrowserRequest {
        method,
        target: parse_host_port(&host, 80)?,
        path: uri.to_string(),
    })
}

fn parse_host_port(value: &str, default_port: u16) -> Result<TargetAddr, String> {
    let value = value.trim();
    if value.starts_with('[') {
        let end = value
            .find(']')
            .ok_or_else(|| format!("invalid IPv6 authority: {value}"))?;
        let host = value[1..end].to_string();
        let port = value[end + 1..]
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Ok(TargetAddr { host, port });
    }
    if let Some((host, port)) = value.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            return Ok(TargetAddr {
                host: host.to_string(),
                port,
            });
        }
    }
    Ok(TargetAddr {
        host: value.to_string(),
        port: default_port,
    })
}

fn header_value(text: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}:");
    text.lines().find_map(|line| {
        line.get(..prefix.len())
            .filter(|value| value.eq_ignore_ascii_case(&prefix))
            .map(|_| line[prefix.len()..].trim().to_string())
    })
}

fn rewrite_http_request(head: &[u8], request: &BrowserRequest) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let version = first.split_whitespace().nth(2).unwrap_or("HTTP/1.1");
    let mut rewritten = format!("{} {} {}\r\n", request.method, request.path, version);
    for line in lines {
        if line.is_empty() {
            rewritten.push_str("\r\n");
            break;
        }
        if !line
            .get(..17)
            .map(|prefix| prefix.eq_ignore_ascii_case("proxy-connection"))
            .unwrap_or(false)
        {
            rewritten.push_str(line);
            rewritten.push_str("\r\n");
        }
    }
    rewritten.into_bytes()
}

async fn connect_via_upstream(
    upstream: &UpstreamProxy,
    target: &TargetAddr,
    timeout: Duration,
) -> Result<Box<dyn ProxyIo>, String> {
    match upstream.scheme {
        ProxyScheme::Http => {
            let stream = tokio::time::timeout(timeout, connect_http_proxy(upstream, target))
                .await
                .map_err(|_| "timed out connecting to HTTP upstream proxy".to_string())??;
            Ok(Box::new(stream))
        }
        ProxyScheme::Socks5 => {
            let proxy_addr = (upstream.host.as_str(), upstream.port);
            let target_addr = (target.host.as_str(), target.port);
            let stream = if let Some(username) = upstream.username.as_deref() {
                Socks5Stream::connect_with_password(
                    proxy_addr,
                    target_addr,
                    username,
                    upstream.password.as_deref().unwrap_or(""),
                )
                .await
            } else {
                Socks5Stream::connect(proxy_addr, target_addr).await
            }
            .map_err(|err| format!("SOCKS upstream connect failed: {err}"))?;
            Ok(Box::new(stream.into_inner()))
        }
    }
}

async fn connect_http_proxy(
    upstream: &UpstreamProxy,
    target: &TargetAddr,
) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect((upstream.host.as_str(), upstream.port))
        .await
        .map_err(|err| format!("HTTP upstream TCP connect failed: {err}"))?;
    let mut request = format!(
        "CONNECT {} HTTP/1.1\r\nHost: {}\r\n",
        target.label(),
        target.label()
    );
    if let Some(header) = upstream.proxy_authorization() {
        request.push_str(&format!("Proxy-Authorization: {header}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("failed to send CONNECT to upstream proxy: {err}"))?;
    let mut response = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|err| format!("failed to read upstream proxy response: {err}"))?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if response.len() > 32 * 1024 {
            return Err("upstream proxy response header is larger than 32KiB".to_string());
        }
    }
    let status_line = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    if !status_line.contains(" 200 ") {
        return Err(format!("upstream proxy CONNECT failed: {status_line}"));
    }
    Ok(stream)
}

impl UpstreamProxy {
    fn proxy_authorization(&self) -> Option<String> {
        self.username.as_ref().map(|username| {
            let password = self.password.as_deref().unwrap_or("");
            format!(
                "Basic {}",
                BASE64_STANDARD.encode(format!("{username}:{password}"))
            )
        })
    }
}

fn parse_upstream_proxy(value: &str) -> Result<UpstreamProxy, String> {
    let url = Url::parse(value).map_err(|err| format!("invalid upstream proxy URL: {err}"))?;
    let scheme = match url.scheme() {
        "http" => ProxyScheme::Http,
        "socks5" | "socks5h" => ProxyScheme::Socks5,
        scheme => return Err(format!("unsupported upstream proxy scheme: {scheme}")),
    };
    let host = url
        .host_str()
        .ok_or_else(|| "upstream proxy URL has no host".to_string())?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "upstream proxy URL has no port".to_string())?;
    let username = (!url.username().is_empty()).then(|| url.username().to_string());
    let password = url.password().map(str::to_string);
    let redacted = redact_proxy_url(&url);
    Ok(UpstreamProxy {
        scheme,
        host,
        port,
        username,
        password,
        redacted,
    })
}

fn redact_proxy_url(url: &Url) -> String {
    let mut redacted = url.clone();
    if !redacted.username().is_empty() {
        let _ = redacted.set_username("***");
    }
    if redacted.password().is_some() {
        let _ = redacted.set_password(Some("***"));
    }
    redacted.to_string()
}

async fn send_event(
    tx: &mpsc::Sender<BridgeEvent>,
    method: &str,
    target: &str,
    upstream: &str,
    result: &str,
    start: Instant,
    error: Option<String>,
) -> Result<(), mpsc::error::SendError<BridgeEvent>> {
    tx.send(BridgeEvent {
        method: method.to_string(),
        target: target.to_string(),
        upstream: upstream.to_string(),
        result: result.to_string(),
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        error,
    })
    .await
}

fn drain_events(rx: &mut mpsc::Receiver<BridgeEvent>, requests: &mut Vec<BridgeEvent>) {
    while let Ok(event) = rx.try_recv() {
        requests.push(event);
    }
}

fn find_chrome() -> Option<PathBuf> {
    if cfg!(windows) {
        for var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(base) = env::var_os(var) {
                let path = PathBuf::from(base).join("Google\\Chrome\\Application\\chrome.exe");
                if path.exists() {
                    return Some(path);
                }
            }
        }
        find_in_path("chrome.exe")
    } else if cfg!(target_os = "macos") {
        let app = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        if app.exists() {
            Some(app)
        } else {
            find_in_path("google-chrome").or_else(|| find_in_path("chromium"))
        }
    } else {
        [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ]
        .into_iter()
        .find_map(find_in_path)
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| path.exists())
}

fn error_report(
    cli: &Cli,
    local_addr: Option<SocketAddr>,
    chrome_path: Option<PathBuf>,
    upstream_proxy: Option<String>,
    error: String,
) -> ChromeProxyReport {
    ChromeProxyReport {
        target_url: cli.url.clone(),
        upstream_proxy: upstream_proxy.unwrap_or_else(|| cli.proxy.clone()),
        local_proxy: local_addr.map(|addr| format!("http://{addr}")),
        chrome_path: chrome_path.map(|path| path.display().to_string()),
        profile_dir: cli
            .profile_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        mode: if cli.show {
            "visible".to_string()
        } else {
            "headless".to_string()
        },
        resolver_rules: RESOLVER_RULES.to_string(),
        launched: false,
        exit_status: None,
        timed_out: false,
        observed_requests: 0,
        successful_tunnels: 0,
        requests: Vec::new(),
        error: Some(error),
        notes: report_notes(),
    }
}

fn report_notes() -> Vec<String> {
    vec![
        "Chrome is launched with --proxy-server pointing to the local bridge.".to_string(),
        "Host resolver rules map normal hostnames to ~NOTFOUND so browser DNS should not resolve locally."
            .to_string(),
        "A successful tunnel means the browser reached the upstream proxy path for that host."
            .to_string(),
    ]
}

fn output(report: &ChromeProxyReport, mode: OutputMode) {
    if mode.is_json() {
        print_json(report);
    } else {
        print_report(report);
    }
}

fn print_report(report: &ChromeProxyReport) {
    println!();
    println!("Chrome Proxy Bridge");
    println!("  URL: {}", report.target_url);
    println!("  Upstream: {}", report.upstream_proxy);
    println!(
        "  Local Proxy: {}",
        report.local_proxy.as_deref().unwrap_or("--")
    );
    println!(
        "  Chrome: {}",
        report.chrome_path.as_deref().unwrap_or("--")
    );
    println!("  Mode: {}", report.mode);
    println!("  Resolver Rules: {}", report.resolver_rules);
    println!("  Launched: {}", if report.launched { "yes" } else { "no" });
    println!("  Exit: {}", report.exit_status.as_deref().unwrap_or("--"));
    println!("  Observed Requests: {}", report.observed_requests);
    println!("  Successful Tunnels: {}", report.successful_tunnels);
    if let Some(error) = &report.error {
        println!("  Error: {error}");
    }

    if !report.requests.is_empty() {
        println!();
        let rows = report
            .requests
            .iter()
            .map(|event| {
                vec![
                    event.method.clone(),
                    event.target.clone(),
                    event.result.clone(),
                    format!("{:.1}ms", event.elapsed_ms),
                    event.error.clone().unwrap_or_else(|| "--".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["Method", "Target", "Result", "Time", "Error"], &rows);
    }

    println!();
    for note in &report.notes {
        println!("  {note}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connect_request() {
        let request = parse_browser_request(
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
        )
        .unwrap();
        assert_eq!(request.method, "CONNECT");
        assert_eq!(request.target.host, "example.com");
        assert_eq!(request.target.port, 443);
    }

    #[test]
    fn parses_absolute_http_request() {
        let request = parse_browser_request(
            b"GET http://example.com/a?q=1 HTTP/1.1\r\nHost: example.com\r\n\r\n",
        )
        .unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.target.host, "example.com");
        assert_eq!(request.target.port, 80);
        assert_eq!(request.path, "/a?q=1");
    }

    #[test]
    fn parses_and_redacts_upstream_proxy() {
        let proxy = parse_upstream_proxy("socks5h://user:pass@127.0.0.1:7890").unwrap();
        assert_eq!(proxy.scheme, ProxyScheme::Socks5);
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 7890);
        assert!(proxy.redacted.contains("***:***"));
    }

    #[test]
    fn rewrites_absolute_form_to_origin_form() {
        let request = parse_browser_request(
            b"GET http://example.com/a HTTP/1.1\r\nHost: example.com\r\n\r\n",
        )
        .unwrap();
        let rewritten = rewrite_http_request(
            b"GET http://example.com/a HTTP/1.1\r\nHost: example.com\r\n\r\n",
            &request,
        );
        assert!(String::from_utf8_lossy(&rewritten).starts_with("GET /a HTTP/1.1\r\n"));
    }
}
