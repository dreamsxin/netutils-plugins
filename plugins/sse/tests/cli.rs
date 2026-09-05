//! CLI 层集成测试：验证输出契约，不依赖外网。
//!
//! 探测目标固定为 `127.0.0.1:1`（保留端口，必然连接失败），因此在离线环境和
//! CI 的三个平台上都能稳定复现，同时仍会走完整的报告渲染路径。

use std::process::{Command, Output};

const DEAD_ENDPOINT: &str = "http://127.0.0.1:1/events";

fn sse(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_netutils-sse"));
    command.args(args);
    // 清掉可能从宿主环境继承的颜色与代理设置，让断言只反映被测参数。
    for name in [
        "NETUTILS_COLOR",
        "NETUTILS_OUTPUT",
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
        "NETUTILS_EFFECTIVE_PROXY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("failed to run netutils-sse")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

const ANSI_PREFIX: &str = "\x1b[";

#[test]
fn help_documents_color_and_proxy() {
    let output = sse(&["--help"], &[]);

    assert!(output.status.success());
    let text = stdout(&output);
    for flag in ["--color", "--proxy", "--no-proxy", "--json"] {
        assert!(text.contains(flag), "help should mention {flag}");
    }
}

#[test]
fn color_flag_rejects_unknown_value() {
    let output = sse(&["--color", "rainbow", DEAD_ENDPOINT], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn color_never_suppresses_ansi() {
    let output = sse(&["--color", "never", "--timeout", "1", DEAD_ENDPOINT], &[]);

    assert!(!stdout(&output).contains(ANSI_PREFIX));
}

#[test]
fn color_always_emits_ansi_even_when_piped() {
    let output = sse(&["--color", "always", "--timeout", "1", DEAD_ENDPOINT], &[]);

    assert!(stdout(&output).contains(ANSI_PREFIX));
}

#[test]
fn piped_output_is_plain_by_default() {
    // stdout 不是终端时不应写入 ANSI 转义，这是旧版 SDK 的缺陷。
    let output = sse(&["--timeout", "1", DEAD_ENDPOINT], &[]);

    assert!(!stdout(&output).contains(ANSI_PREFIX));
}

#[test]
fn json_output_is_plain_and_parseable() {
    let output = sse(&["--json", "--timeout", "1", DEAD_ENDPOINT], &[]);

    let text = stdout(&output);
    assert!(!text.contains(ANSI_PREFIX), "JSON must not carry ANSI");
    assert!(text.trim_start().starts_with('{'), "stdout: {text}");
    assert!(text.contains("\"error\""));
}

#[test]
fn core_forwarded_color_is_honored() {
    let output = sse(
        &["--timeout", "1", DEAD_ENDPOINT],
        &[("NETUTILS_COLOR", "always")],
    );

    assert!(stdout(&output).contains(ANSI_PREFIX));
}

#[test]
fn plugin_flag_overrides_core_forwarded_color() {
    let output = sse(
        &["--color", "never", "--timeout", "1", DEAD_ENDPOINT],
        &[("NETUTILS_COLOR", "always")],
    );

    assert!(!stdout(&output).contains(ANSI_PREFIX));
}

#[test]
fn core_forwarded_json_mode_is_honored() {
    let output = sse(
        &["--timeout", "1", DEAD_ENDPOINT],
        &[("NETUTILS_OUTPUT", "json")],
    );

    assert!(stdout(&output).trim_start().starts_with('{'));
}

#[test]
fn unreachable_endpoint_exits_with_failure() {
    let output = sse(&["--timeout", "1", DEAD_ENDPOINT], &[]);

    assert_eq!(output.status.code(), Some(1));
}
