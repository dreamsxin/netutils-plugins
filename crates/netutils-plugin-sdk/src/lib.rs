//! netutils 插件公共 SDK。
//!
//! 这里定义的是插件与核心之间的**契约**：输出模式、颜色、代理和脱敏行为
//! 必须与 `netutils` 核心保持一致，否则同一个开关在核心和插件上表现不同。
//!
//! 核心通过环境变量向插件传递已解析好的上下文：
//!
//! - `NETUTILS_OUTPUT=json` —— 输出模式
//! - `NETUTILS_COLOR=always|never` —— 核心已解析完成的颜色决定
//! - `NETUTILS_EFFECTIVE_PROXY` —— 针对目标选定的代理
//! - `NETUTILS_CORE_VERSION` / `NETUTILS_PLUGIN_NAME` —— 调用方身份

use std::env;
use std::fmt;
use std::io::IsTerminal;

use serde::Serialize;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

impl OutputMode {
    pub fn from_json_flag(json: bool) -> Self {
        let env_json = env::var("NETUTILS_OUTPUT")
            .map(|value| value.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if json || env_json {
            Self::Json
        } else {
            Self::Human
        }
    }

    pub fn is_json(self) -> bool {
        self == Self::Json
    }
}

/// 颜色请求。这是**意图**，不是最终结果；最终结果由 [`color_enabled`] 给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// 自动判断（默认）
    #[default]
    Auto,
    /// 始终上色
    Always,
    /// 从不上色
    Never,
}

impl ColorMode {
    /// 解析 `auto`/`always`/`never`（大小写不敏感），无法识别时返回 `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "always" | "1" | "true" => Some(Self::Always),
            "never" | "0" | "false" => Some(Self::Never),
            _ => None,
        }
    }

    /// 读取核心下发的 `NETUTILS_COLOR`；未设置或无法识别时返回 `None`。
    pub fn from_core_env() -> Option<Self> {
        Self::parse(&env::var("NETUTILS_COLOR").ok()?)
    }
}

/// 供插件直接用作 clap `value_parser` 的解析函数，避免每个插件各写一份。
///
/// ```ignore
/// #[arg(long, value_name = "WHEN", value_parser = netutils_plugin_sdk::parse_color_arg)]
/// color: Option<netutils_plugin_sdk::ColorMode>,
/// ```
pub fn parse_color_arg(value: &str) -> std::result::Result<ColorMode, String> {
    ColorMode::parse(value)
        .ok_or_else(|| format!("invalid color '{value}', expected auto, always, or never"))
}

impl fmt::Display for ColorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ColorMode::Auto => "auto",
            ColorMode::Always => "always",
            ColorMode::Never => "never",
        })
    }
}

/// 判定是否应该输出 ANSI 颜色。
///
/// 优先级与 `netutils` 核心的 `--color` 实现一一对应：
///
/// 1. 插件自身的 `--color`（`requested`）
/// 2. 核心下发的 `NETUTILS_COLOR`
/// 3. JSON 输出模式强制关闭（ANSI 会破坏解析）
/// 4. `NO_COLOR`（<https://no-color.org>，设为非空即生效）
/// 5. `CLICOLOR_FORCE` 非 `0`
/// 6. `CLICOLOR=0`
/// 7. auto：stdout 是终端时启用
///
/// 把核心下发值放在 `NO_COLOR` 之前是有意为之：核心在解析时已经考虑过
/// `NO_COLOR`，此处再判一次会让 `netutils --color always <plugin>` 失效。
pub fn color_enabled(requested: Option<ColorMode>, output: OutputMode) -> bool {
    resolve_color(
        requested,
        output,
        std::io::stdout().is_terminal(),
        &|name| env::var(name).ok(),
    )
}

fn resolve_color(
    requested: Option<ColorMode>,
    output: OutputMode,
    stdout_is_terminal: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> bool {
    match requested {
        Some(ColorMode::Always) => return true,
        Some(ColorMode::Never) => return false,
        Some(ColorMode::Auto) | None => {}
    }

    match lookup("NETUTILS_COLOR")
        .as_deref()
        .and_then(ColorMode::parse)
    {
        Some(ColorMode::Always) => return true,
        Some(ColorMode::Never) => return false,
        Some(ColorMode::Auto) | None => {}
    }

    if output.is_json() {
        return false;
    }

    if lookup("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return false;
    }

    if lookup("CLICOLOR_FORCE").is_some_and(|value| !value.is_empty() && value != "0") {
        return true;
    }

    if lookup("CLICOLOR").is_some_and(|value| value == "0") {
        return false;
    }

    stdout_is_terminal
}

/// 调用本插件的核心版本（`NETUTILS_CORE_VERSION`）；独立运行时为 `None`。
pub fn core_version() -> Option<String> {
    non_empty_env("NETUTILS_CORE_VERSION")
}

/// 核心分发时使用的插件命令名（`NETUTILS_PLUGIN_NAME`）；独立运行时为 `None`。
pub fn plugin_name() -> Option<String> {
    non_empty_env("NETUTILS_PLUGIN_NAME")
}

/// 是否由 `netutils` 核心分发执行，而非用户直接运行二进制。
pub fn invoked_by_core() -> bool {
    core_version().is_some()
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

#[derive(Debug)]
pub struct PluginError {
    message: String,
}

impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PluginError {}

pub type Result<T> = std::result::Result<T, PluginError>;

pub fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(err) => println!(
            "{}",
            serde_json::json!({
                "error": format!("failed to serialize JSON output: {err}")
            })
        ),
    }
}

pub fn redact_url_credentials(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let Some((_, endpoint)) = rest.rsplit_once('@') else {
        return value.to_string();
    };
    format!("{scheme}://***@{endpoint}")
}

pub fn redact_header_value(name: &str, value: &str) -> String {
    let name = name.to_ascii_lowercase();
    let sensitive = matches!(
        name.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key" | "api-key"
    ) || name.contains("token")
        || name.contains("secret");
    if sensitive {
        "***".to_string()
    } else {
        value.to_string()
    }
}

pub fn proxy_for_url(target: &str, explicit: Option<String>, no_proxy: bool) -> Option<String> {
    if no_proxy {
        return None;
    }
    if explicit.is_some() {
        return explicit;
    }
    if let Ok(value) = env::var("NETUTILS_EFFECTIVE_PROXY") {
        return (!value.is_empty()).then_some(value);
    }
    if proxy_bypassed(target) {
        return None;
    }
    let is_http_like = target
        .split_once("://")
        .map(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("ws"))
        .unwrap_or(false);
    let vars: &[&str] = if is_http_like {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    } else {
        &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
    };
    vars.iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
}

fn proxy_bypassed(target: &str) -> bool {
    let authority = target
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(target)
        .split('/')
        .next()
        .unwrap_or(target);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => host,
            _ => authority,
        }
    }
    .to_ascii_lowercase();
    let Some(rules) = env::var("NO_PROXY").or_else(|_| env::var("no_proxy")).ok() else {
        return false;
    };
    rules
        .split(',')
        .map(|rule| {
            rule.trim()
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .any(|rule| rule == "*" || host == rule || host.ends_with(&format!(".{rule}")))
}

pub fn exit_on_failure(failed: bool) {
    if failed {
        std::process::exit(1);
    }
}

pub fn status_text(ok: bool, color: bool) -> String {
    if ok {
        paint("ok", "32", color)
    } else {
        paint("failed", "31", color)
    }
}

pub fn warn_text(value: &str, color: bool) -> String {
    paint(value, "33", color)
}

pub fn error_text(value: &str, color: bool) -> String {
    paint(value, "31", color)
}

/// `color` 由 [`color_enabled`] 给出，避免各插件各自判断颜色开关。
pub fn paint(value: &str, ansi_code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{ansi_code}m{value}\x1b[0m")
    } else {
        value.to_string()
    }
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths = headers
        .iter()
        .map(|header| UnicodeWidthStr::width(*header))
        .collect::<Vec<_>>();

    for row in rows {
        if row.len() > widths.len() {
            widths.resize(row.len(), 0);
        }
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }

    print_separator(&widths);
    print_row(
        &headers
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
        &widths,
    );
    print_separator(&widths);
    for row in rows {
        print_row(row, &widths);
    }
    print_separator(&widths);
}

fn print_separator(widths: &[usize]) {
    print!("+");
    for width in widths {
        print!("{}+", "-".repeat(width + 2));
    }
    println!();
}

fn print_row(row: &[String], widths: &[usize]) {
    print!("|");
    for (idx, width) in widths.iter().enumerate() {
        let value = row.get(idx).map(String::as_str).unwrap_or("");
        let padding = width.saturating_sub(UnicodeWidthStr::width(value));
        print!(" {value}{} |", " ".repeat(padding));
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn restore_env(name: &str, value: Option<OsString>) {
        match value {
            Some(value) => env::set_var(name, value),
            None => env::remove_var(name),
        }
    }

    #[test]
    fn output_mode_uses_json_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("NETUTILS_OUTPUT");
        assert_eq!(OutputMode::from_json_flag(true), OutputMode::Json);
        assert_eq!(OutputMode::from_json_flag(false), OutputMode::Human);
    }

    #[test]
    fn output_mode_reads_env_protocol() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("NETUTILS_OUTPUT", "json");
        assert_eq!(OutputMode::from_json_flag(false), OutputMode::Json);
        env::remove_var("NETUTILS_OUTPUT");
    }

    #[test]
    fn color_mode_can_disable_paint() {
        assert_eq!(paint("ok", "32", false), "ok");
        assert_eq!(paint("ok", "32", true), "\x1b[32mok\x1b[0m");
    }

    #[test]
    fn color_mode_parses_documented_values() {
        assert_eq!(ColorMode::parse("auto"), Some(ColorMode::Auto));
        assert_eq!(ColorMode::parse("Always"), Some(ColorMode::Always));
        assert_eq!(ColorMode::parse(" NEVER "), Some(ColorMode::Never));
        assert_eq!(ColorMode::parse("rainbow"), None);
    }

    #[test]
    fn color_arg_parser_reports_accepted_values() {
        assert_eq!(parse_color_arg("never"), Ok(ColorMode::Never));
        let err = parse_color_arg("rainbow").unwrap_err();
        assert!(err.contains("auto, always, or never"), "{err}");
    }

    /// 构造一个只读的假环境，避免测试之间互相污染全局 env。
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn plugin_flag_beats_core_env() {
        let env = env_of(&[("NETUTILS_COLOR", "never")]);

        assert!(resolve_color(
            Some(ColorMode::Always),
            OutputMode::Human,
            false,
            &env
        ));
    }

    #[test]
    fn core_env_beats_no_color() {
        // 核心解析 --color 时已经考虑过 NO_COLOR，插件不能再否决一次。
        let env = env_of(&[("NETUTILS_COLOR", "always"), ("NO_COLOR", "1")]);

        assert!(resolve_color(None, OutputMode::Human, false, &env));
    }

    #[test]
    fn core_env_never_disables_color() {
        let env = env_of(&[("NETUTILS_COLOR", "never")]);

        assert!(!resolve_color(None, OutputMode::Human, true, &env));
    }

    #[test]
    fn json_mode_disables_color() {
        let env = env_of(&[]);

        assert!(!resolve_color(None, OutputMode::Json, true, &env));
    }

    #[test]
    fn no_color_disables_color() {
        let env = env_of(&[("NO_COLOR", "1")]);

        assert!(!resolve_color(None, OutputMode::Human, true, &env));
    }

    #[test]
    fn empty_no_color_is_ignored() {
        let env = env_of(&[("NO_COLOR", "")]);

        assert!(resolve_color(None, OutputMode::Human, true, &env));
    }

    #[test]
    fn clicolor_force_enables_without_terminal() {
        let env = env_of(&[("CLICOLOR_FORCE", "1")]);

        assert!(resolve_color(None, OutputMode::Human, false, &env));
    }

    #[test]
    fn clicolor_zero_disables_color() {
        let env = env_of(&[("CLICOLOR", "0")]);

        assert!(!resolve_color(None, OutputMode::Human, true, &env));
    }

    #[test]
    fn auto_follows_terminal_detection() {
        let env = env_of(&[]);

        assert!(resolve_color(None, OutputMode::Human, true, &env));
        // 重定向到文件时不再写入 ANSI 转义。
        assert!(!resolve_color(None, OutputMode::Human, false, &env));
    }

    #[test]
    fn core_identity_is_absent_when_run_standalone() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_version = env::var_os("NETUTILS_CORE_VERSION");
        let old_name = env::var_os("NETUTILS_PLUGIN_NAME");

        env::remove_var("NETUTILS_CORE_VERSION");
        env::remove_var("NETUTILS_PLUGIN_NAME");
        assert_eq!(core_version(), None);
        assert_eq!(plugin_name(), None);
        assert!(!invoked_by_core());

        env::set_var("NETUTILS_CORE_VERSION", "0.4.0");
        env::set_var("NETUTILS_PLUGIN_NAME", "sse");
        assert_eq!(core_version().as_deref(), Some("0.4.0"));
        assert_eq!(plugin_name().as_deref(), Some("sse"));
        assert!(invoked_by_core());

        // 空值等同于未设置，避免把空字符串当成有效身份。
        env::set_var("NETUTILS_CORE_VERSION", "");
        assert_eq!(core_version(), None);

        restore_env("NETUTILS_CORE_VERSION", old_version);
        restore_env("NETUTILS_PLUGIN_NAME", old_name);
    }

    #[test]
    fn redacts_credentials_and_headers() {
        assert_eq!(
            redact_url_credentials("socks5h://user:pass@example.com:1080"),
            "socks5h://***@example.com:1080"
        );
        assert_eq!(redact_header_value("Authorization", "Bearer secret"), "***");
    }

    #[test]
    fn explicit_proxy_has_priority() {
        assert_eq!(
            proxy_for_url(
                "https://example.com",
                Some("socks5h://127.0.0.1:1080".to_string()),
                false
            )
            .as_deref(),
            Some("socks5h://127.0.0.1:1080")
        );
        assert!(proxy_for_url(
            "https://example.com",
            Some("socks5h://127.0.0.1:1080".to_string()),
            true
        )
        .is_none());
    }

    #[test]
    fn websocket_urls_use_matching_proxy_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_http = env::var_os("HTTP_PROXY");
        let old_https = env::var_os("HTTPS_PROXY");
        let old_no_proxy = env::var_os("NO_PROXY");
        env::set_var("HTTP_PROXY", "http://plain-proxy:8080");
        env::set_var("HTTPS_PROXY", "http://tls-proxy:8080");
        env::remove_var("NO_PROXY");
        assert_eq!(
            proxy_for_url("ws://example.com/socket", None, false),
            Some("http://plain-proxy:8080".to_string())
        );
        assert_eq!(
            proxy_for_url("wss://example.com/socket", None, false),
            Some("http://tls-proxy:8080".to_string())
        );
        restore_env("HTTP_PROXY", old_http);
        restore_env("HTTPS_PROXY", old_https);
        restore_env("NO_PROXY", old_no_proxy);
    }
}
