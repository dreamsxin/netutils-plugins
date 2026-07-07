use std::time::{Duration, Instant};

use clap::Parser;
use colored::*;
use netutils_plugin_sdk::{print_json, OutputMode};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde::Serialize;
use serde_json::{json, Value};

const MCP_SESSION_ID: &str = "MCP-Session-Id";
const MCP_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";

#[derive(Parser, Debug)]
#[command(
    name = "netutils-mcp",
    version,
    about = "MCP Streamable HTTP diagnostics"
)]
struct Cli {
    /// JSON output
    #[arg(long)]
    json: bool,

    /// MCP Streamable HTTP endpoint, for example https://example.com/mcp
    url: String,

    /// Compatibility with old dispatchers that forwarded `mcp <url>` to the plugin.
    #[arg(hide = true)]
    forwarded_url: Option<String>,

    /// Request header, repeatable, for example -H "Authorization: Bearer xxx"
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Request/connect timeout seconds
    #[arg(long, default_value_t = 10)]
    timeout: u64,

    /// MCP protocol version
    #[arg(long, default_value = "2025-11-25")]
    protocol_version: String,

    /// Do not run tools/list
    #[arg(long)]
    no_tools: bool,

    /// Call a tool after initialization
    #[arg(long)]
    tool: Option<String>,

    /// JSON object passed as tools/call arguments
    #[arg(long = "args", default_value = "{}")]
    tool_args: String,

    /// Require the tool to be present in tools/list before calling it
    #[arg(long)]
    require_tool: bool,

    /// Open GET server-to-client SSE stream after initialization
    #[arg(long)]
    listen: bool,

    /// Max SSE events to collect in listen mode
    #[arg(long, default_value_t = 5)]
    max_events: usize,

    /// Max seconds to listen
    #[arg(long, default_value_t = 30)]
    max_seconds: u64,

    /// HTTP/SOCKS proxy
    #[arg(long)]
    proxy: Option<String>,

    /// Force direct access and ignore proxies
    #[arg(long, alias = "no-system-proxy")]
    no_proxy: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    url: String,
    protocol_version: String,
    proxy: ProxyInfo,
    session_id: Option<String>,
    initialized: bool,
    initialize: Exchange,
    initialized_notification: Option<Exchange>,
    tools_list: Option<Exchange>,
    tool_call: Option<Exchange>,
    listen: Option<ListenResult>,
    summary: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProxyInfo {
    mode: String,
    value: Option<String>,
}

#[derive(Debug, Serialize)]
struct Exchange {
    name: String,
    status: Option<u16>,
    content_type: Option<String>,
    ok: bool,
    elapsed_ms: f64,
    session_id: Option<String>,
    response: Option<Value>,
    sse_messages: Vec<Value>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListenResult {
    status: Option<u16>,
    content_type: Option<String>,
    connected: bool,
    events: Vec<SseEvent>,
    error: Option<String>,
    elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
struct SseEvent {
    index: usize,
    event: Option<String>,
    id: Option<String>,
    data: String,
    json: Option<Value>,
}

#[derive(Default)]
struct SseEventBuilder {
    event: Option<String>,
    id: Option<String>,
    data_lines: Vec<String>,
}

#[derive(Default)]
struct SseParser {
    buffer: String,
    current: SseEventBuilder,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let timeout = Duration::from_secs(cli.timeout);
    let max_duration = Duration::from_secs(cli.max_seconds);
    let url = match effective_url(&cli) {
        Ok(url) => normalize_url(url),
        Err(err) => {
            output(error_report(&cli.url, &cli.protocol_version, err), cli.json);
            return;
        }
    };

    let headers = match parse_headers(&cli.headers) {
        Ok(headers) => headers,
        Err(err) => {
            output(error_report(&url, &cli.protocol_version, err), cli.json);
            return;
        }
    };
    let tool_args = match parse_tool_arguments(&cli.tool_args) {
        Ok(args) => args,
        Err(err) => {
            output(error_report(&url, &cli.protocol_version, err), cli.json);
            return;
        }
    };

    let proxy = proxy_info(cli.proxy.clone(), cli.no_proxy);
    let client = match build_client(timeout, cli.proxy.as_deref().filter(|_| !cli.no_proxy)) {
        Ok(client) => client,
        Err(err) => {
            output(
                error_report(
                    &url,
                    &cli.protocol_version,
                    format!("failed to build client: {err}"),
                ),
                cli.json,
            );
            return;
        }
    };

    let initialize = post_json_rpc(
        &client,
        &url,
        &headers,
        None,
        None,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": cli.protocol_version,
                "capabilities": {},
                "clientInfo": {
                    "name": "netutils-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        Some(1),
        timeout,
        "initialize",
    )
    .await;

    let session_id = initialize.session_id.clone();
    let initialized = initialize.ok && initialize.error.is_none();
    let initialized_notification = if initialized {
        Some(
            post_json_rpc(
                &client,
                &url,
                &headers,
                session_id.as_deref(),
                Some(&cli.protocol_version),
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }),
                None,
                timeout,
                "notifications/initialized",
            )
            .await,
        )
    } else {
        None
    };

    let tools_list = if initialized && !cli.no_tools {
        Some(
            post_json_rpc(
                &client,
                &url,
                &headers,
                session_id.as_deref(),
                Some(&cli.protocol_version),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list",
                    "params": {}
                }),
                Some(2),
                timeout,
                "tools/list",
            )
            .await,
        )
    } else {
        None
    };

    let tool_call = if initialized {
        match cli.tool.as_deref() {
            Some(tool_name) if cli.require_tool && tools_list.is_none() => Some(failed_exchange(
                &format!("tools/call {tool_name}"),
                "--require-tool needs tools/list; remove --no-tools or --require-tool".to_string(),
            )),
            Some(tool_name)
                if cli.require_tool
                    && tools_list
                        .as_ref()
                        .map(|tools| !contains_tool(tools, tool_name))
                        .unwrap_or(true) =>
            {
                Some(failed_exchange(
                    &format!("tools/call {tool_name}"),
                    format!("tool not found in tools/list: {tool_name}"),
                ))
            }
            Some(tool_name) => Some(
                post_json_rpc(
                    &client,
                    &url,
                    &headers,
                    session_id.as_deref(),
                    Some(&cli.protocol_version),
                    json!({
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/call",
                        "params": {
                            "name": tool_name,
                            "arguments": tool_args
                        }
                    }),
                    Some(3),
                    timeout,
                    &format!("tools/call {tool_name}"),
                )
                .await,
            ),
            None => None,
        }
    } else {
        None
    };

    let listen = if initialized && cli.listen {
        Some(
            listen_stream(
                &client,
                &url,
                &headers,
                session_id.as_deref(),
                &cli.protocol_version,
                cli.max_events,
                max_duration,
            )
            .await,
        )
    } else {
        None
    };

    output(
        report(
            url,
            cli.protocol_version,
            proxy,
            session_id,
            initialize,
            initialized_notification,
            tools_list,
            tool_call,
            listen,
        ),
        cli.json,
    );
}

async fn post_json_rpc(
    client: &reqwest::Client,
    url: &str,
    base_headers: &HeaderMap,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    body: Value,
    expected_id: Option<i64>,
    timeout: Duration,
    name: &str,
) -> Exchange {
    let start = Instant::now();
    let request = apply_mcp_headers(
        client
            .post(url)
            .headers(base_headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(body.to_string()),
        session_id,
        protocol_version,
    );
    let response = match tokio::time::timeout(timeout, request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => return failed_timed_exchange(name, err.to_string(), start.elapsed()),
        Err(_) => {
            return failed_timed_exchange(name, "request timeout".to_string(), start.elapsed())
        }
    };

    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let response_session_id = headers
        .get(MCP_SESSION_ID)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let lower_type = content_type
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if status == 202 {
        return Exchange {
            name: name.to_string(),
            status: Some(status),
            content_type,
            ok: true,
            elapsed_ms: elapsed_ms(start),
            session_id: response_session_id,
            response: None,
            sse_messages: Vec::new(),
            error: None,
        };
    }

    if lower_type.contains("text/event-stream") {
        let (messages, error) = read_sse_json_messages(response, expected_id, timeout).await;
        let ok = status < 400
            && error.is_none()
            && messages.iter().any(|message| {
                expected_id
                    .map(|id| message.get("id") == Some(&json!(id)))
                    .unwrap_or(true)
            });
        return Exchange {
            name: name.to_string(),
            status: Some(status),
            content_type,
            ok,
            elapsed_ms: elapsed_ms(start),
            session_id: response_session_id,
            response: None,
            sse_messages: messages,
            error,
        };
    }

    let text = match tokio::time::timeout(timeout, response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(err)) => return failed_timed_exchange(name, err.to_string(), start.elapsed()),
        Err(_) => return failed_timed_exchange(name, "read timeout".to_string(), start.elapsed()),
    };
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let error = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .map(|error| error.to_string())
        .or_else(|| {
            if parsed.is_none() && !text.is_empty() {
                Some(format!("non-JSON response body: {}", truncate(&text, 240)))
            } else {
                None
            }
        });
    Exchange {
        name: name.to_string(),
        status: Some(status),
        content_type,
        ok: status < 400 && error.is_none(),
        elapsed_ms: elapsed_ms(start),
        session_id: response_session_id,
        response: parsed,
        sse_messages: Vec::new(),
        error,
    }
}

async fn read_sse_json_messages(
    mut response: reqwest::Response,
    expected_id: Option<i64>,
    timeout: Duration,
) -> (Vec<Value>, Option<String>) {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut parser = SseParser::default();
    let mut messages = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                for event in parser.push(&chunk) {
                    if let Some(value) = event.json {
                        let is_expected = expected_id
                            .map(|id| value.get("id") == Some(&json!(id)))
                            .unwrap_or(true);
                        messages.push(value);
                        if is_expected {
                            return (messages, None);
                        }
                    }
                }
            }
            Ok(Ok(None)) => {
                for event in parser.finish() {
                    if let Some(value) = event.json {
                        messages.push(value);
                    }
                }
                return (messages, None);
            }
            Ok(Err(err)) => return (messages, Some(err.to_string())),
            Err(_) => return (messages, Some("SSE read timeout".to_string())),
        }
    }
}

async fn listen_stream(
    client: &reqwest::Client,
    url: &str,
    base_headers: &HeaderMap,
    session_id: Option<&str>,
    protocol_version: &str,
    max_events: usize,
    max_duration: Duration,
) -> ListenResult {
    let start = Instant::now();
    let request = apply_mcp_headers(
        client
            .get(url)
            .headers(base_headers.clone())
            .header(ACCEPT, "text/event-stream"),
        session_id,
        Some(protocol_version),
    );
    let response = match tokio::time::timeout(max_duration, request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => return failed_listen(err.to_string(), start.elapsed()),
        Err(_) => return failed_listen("GET stream timeout".to_string(), start.elapsed()),
    };

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut response = response;
    let deadline = tokio::time::Instant::now() + max_duration;
    let mut parser = SseParser::default();
    let mut events = Vec::new();
    let mut error = None;
    while events.len() < max_events && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                events.extend(parser.push(&chunk));
                if events.len() > max_events {
                    events.truncate(max_events);
                }
            }
            Ok(Ok(None)) => {
                events.extend(parser.finish());
                break;
            }
            Ok(Err(err)) => {
                error = Some(err.to_string());
                break;
            }
            Err(_) => break,
        }
    }
    ListenResult {
        status: Some(status),
        content_type,
        connected: status < 400,
        events: renumber(events),
        error,
        elapsed_ms: elapsed_ms(start),
    }
}

fn apply_mcp_headers(
    mut request: reqwest::RequestBuilder,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
) -> reqwest::RequestBuilder {
    if let Some(session_id) = session_id {
        request = request.header(MCP_SESSION_ID, session_id);
    }
    if let Some(protocol_version) = protocol_version {
        request = request.header(MCP_PROTOCOL_VERSION, protocol_version);
    }
    request
}

fn build_client(timeout: Duration, proxy: Option<&str>) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("netutils-mcp");
    if let Some(proxy_url) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    } else {
        builder = builder.no_proxy();
    }
    builder.build()
}

fn parse_headers(raw_headers: &[String]) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
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
    Ok(headers)
}

fn parse_tool_arguments(raw: &str) -> Result<Value, String> {
    let value = serde_json::from_str::<Value>(raw)
        .map_err(|err| format!("invalid --args JSON object: {err}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err("--args must be a JSON object".to_string())
    }
}

impl SseParser {
    fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let mut line = self.buffer[..pos].to_string();
            self.buffer.drain(..=pos);
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event) = self.parse_line(&line) {
                events.push(event);
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.parse_line(line.trim_end_matches('\r')) {
                events.push(event);
            }
        }
        if self.current.has_data() {
            events.push(self.current.take_event());
        }
        events
    }

    fn parse_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.current.has_data().then(|| self.current.take_event());
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.current.event = Some(value.to_string()),
            "id" => self.current.id = Some(value.to_string()),
            "data" => self.current.data_lines.push(value.to_string()),
            _ => {}
        }
        None
    }
}

impl SseEventBuilder {
    fn has_data(&self) -> bool {
        self.event.is_some() || self.id.is_some() || !self.data_lines.is_empty()
    }

    fn take_event(&mut self) -> SseEvent {
        let data = self.data_lines.join("\n");
        let json = serde_json::from_str::<Value>(&data).ok();
        let event = SseEvent {
            index: 0,
            event: self.event.take(),
            id: self.id.take(),
            data,
            json,
        };
        self.data_lines.clear();
        event
    }
}

fn normalize_url(input: &str) -> String {
    if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

fn effective_url(cli: &Cli) -> Result<&str, String> {
    match cli.forwarded_url.as_deref() {
        Some(url) if cli.url == "mcp" => Ok(url),
        Some(extra) => Err(format!("unexpected extra argument: {extra}")),
        None => Ok(&cli.url),
    }
}

fn proxy_info(value: Option<String>, no_proxy: bool) -> ProxyInfo {
    ProxyInfo {
        mode: if no_proxy {
            "direct-forced".to_string()
        } else if value.is_some() {
            "proxy".to_string()
        } else {
            "direct".to_string()
        },
        value,
    }
}

fn report(
    url: String,
    protocol_version: String,
    proxy: ProxyInfo,
    session_id: Option<String>,
    initialize: Exchange,
    initialized_notification: Option<Exchange>,
    tools_list: Option<Exchange>,
    tool_call: Option<Exchange>,
    listen: Option<ListenResult>,
) -> Report {
    let initialized = initialize.ok
        && initialized_notification
            .as_ref()
            .map(|exchange| exchange.ok)
            .unwrap_or(false);
    let mut summary = Vec::new();
    summary.push(if initialize.ok {
        "initialize succeeded".to_string()
    } else {
        "initialize failed".to_string()
    });
    if let Some(session_id) = &session_id {
        summary.push(format!("session id received: {session_id}"));
    }
    if let Some(tools) = &tools_list {
        summary.push(if tools.ok {
            format!("tools/list succeeded ({} tool(s))", count_tools(tools))
        } else {
            "tools/list failed".to_string()
        });
    }
    if let Some(call) = &tool_call {
        summary.push(if call.ok {
            format!("{} succeeded", call.name)
        } else {
            format!("{} failed", call.name)
        });
    }
    Report {
        url,
        protocol_version,
        proxy,
        session_id,
        initialized,
        initialize,
        initialized_notification,
        tools_list,
        tool_call,
        listen,
        summary,
        notes: vec![
            "MCP Streamable HTTP uses POST for JSON-RPC and may return either application/json or text/event-stream.".to_string(),
            "If a session id is returned during initialize, subsequent requests include MCP-Session-Id.".to_string(),
        ],
    }
}

fn error_report(url: &str, protocol_version: &str, error: String) -> Report {
    report(
        url.to_string(),
        protocol_version.to_string(),
        proxy_info(None, true),
        None,
        failed_exchange("initialize", error),
        None,
        None,
        None,
        None,
    )
}

fn count_tools(exchange: &Exchange) -> usize {
    exchange
        .response
        .as_ref()
        .or_else(|| {
            exchange
                .sse_messages
                .iter()
                .find(|value| value.get("result").is_some())
        })
        .and_then(|value| value.pointer("/result/tools"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn contains_tool(exchange: &Exchange, name: &str) -> bool {
    tool_list_value(exchange)
        .and_then(|value| value.pointer("/result/tools"))
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
        })
        .unwrap_or(false)
}

fn tool_list_value(exchange: &Exchange) -> Option<&Value> {
    exchange.response.as_ref().or_else(|| {
        exchange
            .sse_messages
            .iter()
            .find(|value| value.get("result").is_some())
    })
}

fn exchange_payload_value(exchange: &Exchange) -> Option<&Value> {
    exchange.response.as_ref().or_else(|| {
        exchange
            .sse_messages
            .iter()
            .find(|value| value.get("result").is_some() || value.get("error").is_some())
    })
}

fn renumber(events: Vec<SseEvent>) -> Vec<SseEvent> {
    events
        .into_iter()
        .enumerate()
        .map(|(idx, mut event)| {
            event.index = idx + 1;
            event
        })
        .collect()
}

fn failed_exchange(name: &str, error: String) -> Exchange {
    failed_timed_exchange(name, error, Duration::ZERO)
}

fn failed_timed_exchange(name: &str, error: String, elapsed: Duration) -> Exchange {
    Exchange {
        name: name.to_string(),
        status: None,
        content_type: None,
        ok: false,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        session_id: None,
        response: None,
        sse_messages: Vec::new(),
        error: Some(error),
    }
}

fn failed_listen(error: String, elapsed: Duration) -> ListenResult {
    ListenResult {
        status: None,
        content_type: None,
        connected: false,
        events: Vec::new(),
        error: Some(error),
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    }
}

fn output(report: Report, json: bool) {
    match OutputMode::from_json_flag(json) {
        OutputMode::Json => print_json(&report),
        OutputMode::Human => print_report(&report),
    }
}

fn print_report(report: &Report) {
    println!();
    println!("{}", "MCP Streamable HTTP".bold());
    println!("  URL: {}", report.url);
    println!("  Protocol: {}", report.protocol_version);
    println!(
        "  Session: {}",
        report.session_id.as_deref().unwrap_or("--")
    );
    println!(
        "  Initialized: {}",
        if report.initialized {
            "yes".green().to_string()
        } else {
            "no".red().to_string()
        }
    );
    println!();
    print_exchange(&report.initialize);
    if let Some(exchange) = &report.initialized_notification {
        print_exchange(exchange);
    }
    if let Some(exchange) = &report.tools_list {
        print_exchange(exchange);
        print_tools(exchange);
    }
    if let Some(exchange) = &report.tool_call {
        print_exchange(exchange);
        print_tool_call(exchange);
    }
    if let Some(listen) = &report.listen {
        println!();
        println!("{}", "Listen Stream".bold());
        println!("  Connected: {}", listen.connected);
        println!(
            "  Status: {}",
            listen
                .status
                .map(|status| status.to_string())
                .unwrap_or_else(|| "--".to_string())
        );
        println!("  Events: {}", listen.events.len());
        if let Some(error) = &listen.error {
            println!("  Error: {error}");
        }
    }
    println!();
    for item in &report.summary {
        println!("  {item}");
    }
}

fn print_exchange(exchange: &Exchange) {
    println!();
    println!("{}", exchange.name.bold());
    println!("  OK: {}", exchange.ok);
    println!(
        "  Status: {}",
        exchange
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    println!("  Time: {:.2}ms", exchange.elapsed_ms);
    if let Some(error) = &exchange.error {
        println!("  Error: {}", truncate(error, 240));
    }
}

fn print_tools(exchange: &Exchange) {
    let Some(value) = tool_list_value(exchange) else {
        return;
    };
    let Some(tools) = value.pointer("/result/tools").and_then(Value::as_array) else {
        return;
    };
    if tools.is_empty() {
        return;
    }
    println!();
    println!("{}", "Tools".bold());
    for tool in tools {
        println!(
            "  {} - {}",
            tool.get("name").and_then(Value::as_str).unwrap_or("--"),
            truncate(
                tool.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("--"),
                120
            )
        );
    }
}

fn print_tool_call(exchange: &Exchange) {
    let Some(value) = exchange_payload_value(exchange) else {
        return;
    };
    let formatted = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    println!("  Result: {}", truncate(&formatted, 2000));
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
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
    fn parses_sse_json_messages() {
        let mut parser = SseParser::default();
        let events =
            parser.push(b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].json.as_ref().unwrap().get("id"), Some(&json!(1)));
    }

    #[test]
    fn counts_tools_from_response() {
        let mut exchange = failed_exchange("tools/list", String::new());
        exchange.response = Some(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": { "tools": [{ "name": "a" }, { "name": "b" }] }
        }));
        assert_eq!(count_tools(&exchange), 2);
        assert!(contains_tool(&exchange, "a"));
        assert!(!contains_tool(&exchange, "missing"));
    }

    #[test]
    fn parses_tool_arguments_as_json_object() {
        assert_eq!(parse_tool_arguments("{}").unwrap(), json!({}));
        assert_eq!(
            parse_tool_arguments("{\"action\":\"list\"}").unwrap(),
            json!({ "action": "list" })
        );
        assert!(parse_tool_arguments("[]").is_err());
        assert!(parse_tool_arguments("{").is_err());
    }

    #[test]
    fn accepts_url_from_normal_and_legacy_forwarded_forms() {
        let normal = test_cli("https://example.com/mcp", None);
        assert_eq!(effective_url(&normal).unwrap(), "https://example.com/mcp");

        let legacy = test_cli("mcp", Some("https://example.com/mcp"));
        assert_eq!(effective_url(&legacy).unwrap(), "https://example.com/mcp");
    }

    #[test]
    fn rejects_unexpected_extra_positional_argument() {
        let cli = test_cli("https://example.com/mcp", Some("extra"));
        assert!(effective_url(&cli).is_err());
    }

    fn test_cli(url: &str, forwarded_url: Option<&str>) -> Cli {
        Cli {
            json: false,
            url: url.to_string(),
            forwarded_url: forwarded_url.map(ToString::to_string),
            headers: Vec::new(),
            timeout: 10,
            protocol_version: "2025-11-25".to_string(),
            no_tools: false,
            tool: None,
            tool_args: "{}".to_string(),
            require_tool: false,
            listen: false,
            max_events: 5,
            max_seconds: 30,
            proxy: None,
            no_proxy: false,
        }
    }
}
