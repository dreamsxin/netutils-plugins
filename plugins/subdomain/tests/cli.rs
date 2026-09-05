//! CLI 层集成测试：验证代理契约与输出结构，不依赖外网结果。
//!
//! 这里只断言「报告里如何记录代理选择」，不断言数据源是否可达，
//! 因此在离线和联网环境下结果一致。

use std::process::{Command, Output};

fn subdomain(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_netutils-subdomain"));
    command.args(args);
    for name in [
        "NETUTILS_OUTPUT",
        "NETUTILS_EFFECTIVE_PROXY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        command.env_remove(name);
    }
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("failed to run netutils-subdomain")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// 单个数据源 + 极短超时，把每个用例的耗时压到最低。
const FAST: [&str; 4] = ["--source", "crtsh", "--timeout", "1"];

fn run_json(extra: &[&str], envs: &[(&str, &str)]) -> String {
    let mut args = vec!["--json"];
    args.extend_from_slice(&FAST);
    args.extend_from_slice(extra);
    args.push("example.com");
    stdout(&subdomain(&args, envs))
}

#[test]
fn help_documents_proxy_flags() {
    let output = subdomain(&["--help"], &[]);

    assert!(output.status.success());
    let text = stdout(&output);
    for flag in ["--proxy", "--no-proxy", "--source", "--json"] {
        assert!(text.contains(flag), "help should mention {flag}");
    }
}

#[test]
fn report_records_direct_when_no_proxy_is_configured() {
    let text = run_json(&[], &[]);

    assert!(text.contains("\"proxy\""), "stdout: {text}");
    assert!(text.contains("\"mode\": \"direct\""), "stdout: {text}");
}

#[test]
fn explicit_proxy_is_recorded_and_redacted() {
    let text = run_json(&["--proxy", "http://user:secret@127.0.0.1:1"], &[]);

    assert!(text.contains("\"mode\": \"proxy\""), "stdout: {text}");
    assert!(text.contains("http://***@127.0.0.1:1"), "stdout: {text}");
    assert!(!text.contains("secret"), "credentials must be redacted");
}

#[test]
fn no_proxy_flag_forces_direct_over_environment() {
    // 此前 reqwest 会静默继承环境代理，--no-proxy 形同虚设。
    let text = run_json(&["--no-proxy"], &[("HTTPS_PROXY", "http://127.0.0.1:1")]);

    assert!(
        text.contains("\"mode\": \"direct-forced\""),
        "stdout: {text}"
    );
}

#[test]
fn environment_proxy_is_picked_up_when_allowed() {
    let text = run_json(&[], &[("HTTPS_PROXY", "http://127.0.0.1:1")]);

    assert!(text.contains("\"mode\": \"proxy\""), "stdout: {text}");
}

#[test]
fn core_forwarded_proxy_takes_precedence_over_environment() {
    let text = run_json(
        &[],
        &[
            ("NETUTILS_EFFECTIVE_PROXY", "http://127.0.0.1:2"),
            ("HTTPS_PROXY", "http://127.0.0.1:1"),
        ],
    );

    assert!(text.contains("http://127.0.0.1:2"), "stdout: {text}");
}

#[test]
fn invalid_domain_is_reported_without_network_access() {
    let output = subdomain(&["--json", "not a domain"], &[]);

    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("\"error\""), "stdout: {text}");
    assert!(text.contains("\"proxy\""), "stdout: {text}");
}
