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
        if json {
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

    #[test]
    fn output_mode_uses_json_flag() {
        assert_eq!(OutputMode::from_json_flag(true), OutputMode::Json);
        assert_eq!(OutputMode::from_json_flag(false), OutputMode::Human);
    }

    #[test]
    fn color_mode_can_disable_paint() {
        assert_eq!(paint("ok", "32", ColorMode::Never), "ok");
        assert_eq!(paint("ok", "32", ColorMode::Always), "\x1b[32mok\x1b[0m");
    }
}
