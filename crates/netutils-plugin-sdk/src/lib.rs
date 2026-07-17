use std::env;
use std::fmt;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn from_env() -> Self {
        let color = env::var("NETUTILS_COLOR").unwrap_or_default();
        if env::var_os("NO_COLOR").is_some() || color.eq_ignore_ascii_case("never") {
            Self::Never
        } else if color.eq_ignore_ascii_case("always") {
            Self::Always
        } else {
            Self::Auto
        }
    }

    pub fn enabled(self) -> bool {
        !matches!(self, Self::Never)
    }
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

pub fn status_text(ok: bool, color: ColorMode) -> String {
    if ok {
        paint("ok", "32", color)
    } else {
        paint("failed", "31", color)
    }
}

pub fn warn_text(value: &str, color: ColorMode) -> String {
    paint(value, "33", color)
}

pub fn error_text(value: &str, color: ColorMode) -> String {
    paint(value, "31", color)
}

pub fn paint(value: &str, ansi_code: &str, color: ColorMode) -> String {
    if color.enabled() {
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
        assert_eq!(paint("ok", "32", ColorMode::Never), "ok");
        assert_eq!(paint("ok", "32", ColorMode::Always), "\x1b[32mok\x1b[0m");
    }

    #[test]
    fn color_mode_reads_env_protocol() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("NO_COLOR");
        env::set_var("NETUTILS_COLOR", "always");
        assert_eq!(ColorMode::from_env(), ColorMode::Always);
        env::set_var("NETUTILS_COLOR", "never");
        assert_eq!(ColorMode::from_env(), ColorMode::Never);
        env::remove_var("NETUTILS_COLOR");
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
