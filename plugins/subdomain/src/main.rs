//! Subdomain discovery from public certificate transparency and passive DNS sources.

use std::{collections::BTreeSet, time::Duration};

use clap::{Parser, ValueEnum};
use netutils_plugin_sdk::{print_json, print_table, OutputMode};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(
    name = "netutils-subdomain",
    version,
    about = "Discover subdomains from public passive sources"
)]
struct Cli {
    /// JSON output
    #[arg(long)]
    json: bool,

    /// Domain to discover, for example example.com
    domain: String,

    /// Discovery source
    #[arg(long, value_enum, default_value_t = Source::All)]
    source: Source,

    /// HTTP request timeout seconds
    #[arg(long, default_value_t = 15)]
    timeout: u64,

    /// Maximum subdomains to return
    #[arg(long, default_value_t = 5000)]
    max: usize,

    /// Keep wildcard names instead of stripping the leading "*."
    #[arg(long)]
    include_wildcards: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Source {
    Crtsh,
    Bufferover,
    All,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Self::Crtsh => "crtsh",
            Self::Bufferover => "bufferover",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SubdomainReport {
    pub domain: String,
    pub source: String,
    pub query_url: Option<String>,
    pub discovered: usize,
    pub returned: usize,
    pub truncated: bool,
    pub include_wildcards: bool,
    pub subdomains: Vec<String>,
    pub error: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CrtShRecord {
    name_value: Option<String>,
    common_name: Option<String>,
}

#[derive(Debug)]
struct DiscoveryResult {
    subdomains: Vec<String>,
    query_url: Option<String>,
    error: Option<String>,
    notes: Vec<String>,
}

impl DiscoveryResult {
    fn error(query_url: Option<String>, error: String) -> Self {
        Self {
            subdomains: Vec::new(),
            query_url,
            error: Some(error),
            notes: default_notes(),
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mode = OutputMode::from_json_flag(cli.json);
    let report = run(cli).await;
    let failed = report.error.is_some();
    output(&report, mode);
    if failed {
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> SubdomainReport {
    let domain = match normalize_domain(&cli.domain) {
        Ok(domain) => domain,
        Err(err) => return error_report(&cli.domain, cli.source, cli.include_wildcards, err),
    };
    let timeout = Duration::from_secs(cli.timeout);

    match cli.source {
        Source::Crtsh => {
            let result = discover_crtsh(&domain, timeout, cli.include_wildcards).await;
            build_report(
                &domain,
                Source::Crtsh,
                result,
                cli.include_wildcards,
                cli.max,
            )
        }
        Source::Bufferover => {
            let result = discover_bufferover(&domain, timeout, cli.include_wildcards).await;
            build_report(
                &domain,
                Source::Bufferover,
                result,
                cli.include_wildcards,
                cli.max,
            )
        }
        Source::All => query_all(&domain, timeout, cli.max, cli.include_wildcards).await,
    }
}

async fn query_all(
    domain: &str,
    timeout: Duration,
    max: usize,
    include_wildcards: bool,
) -> SubdomainReport {
    let (crtsh, bufferover) = tokio::join!(
        discover_crtsh(domain, timeout, include_wildcards),
        discover_bufferover(domain, timeout, include_wildcards)
    );

    let mut combined = BTreeSet::new();
    let mut query_urls = Vec::new();
    let mut notes = default_notes();
    let mut errors = Vec::new();

    for (source, result) in [(Source::Crtsh, crtsh), (Source::Bufferover, bufferover)] {
        if let Some(url) = result.query_url {
            query_urls.push(url);
        }
        if let Some(error) = result.error {
            errors.push(format!("{}: {error}", source.as_str()));
            notes.push(format!("{} source failed: {error}", source.as_str()));
        } else {
            for note in result.notes {
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
        combined.extend(result.subdomains);
    }

    let discovered = combined.len();
    let subdomains = combined.into_iter().take(max).collect::<Vec<_>>();
    let error = if subdomains.is_empty() && !errors.is_empty() {
        Some(errors.join(" | "))
    } else {
        None
    };

    SubdomainReport {
        domain: domain.to_string(),
        source: Source::All.as_str().to_string(),
        query_url: (!query_urls.is_empty()).then(|| query_urls.join(" | ")),
        discovered,
        returned: subdomains.len(),
        truncated: discovered > subdomains.len(),
        include_wildcards,
        subdomains,
        error,
        notes,
    }
}

async fn discover_crtsh(
    domain: &str,
    timeout: Duration,
    include_wildcards: bool,
) -> DiscoveryResult {
    let query_url = format!("https://crt.sh/?q=%25.{domain}&output=json");
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("netutils subdomain")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return DiscoveryResult::error(
                Some(query_url),
                format!("failed to build HTTP client: {err}"),
            )
        }
    };

    let records = match fetch_crtsh_records(&client, &query_url).await {
        Ok(records) => records,
        Err(err) => return DiscoveryResult::error(Some(query_url), err),
    };

    DiscoveryResult {
        subdomains: collect_subdomains(&records, domain, include_wildcards),
        query_url: Some(query_url),
        error: None,
        notes: vec![
            "crt.sh returns certificate transparency names, including stale certificate names."
                .to_string(),
        ],
    }
}

async fn fetch_crtsh_records(
    client: &reqwest::Client,
    query_url: &str,
) -> Result<Vec<CrtShRecord>, String> {
    let mut last_error = None;
    for attempt in 0..3 {
        match client.get(query_url).send().await {
            Ok(response) if response.status().is_success() => {
                return response
                    .json::<Vec<CrtShRecord>>()
                    .await
                    .map_err(|err| format!("failed to parse crt.sh JSON: {err}"));
            }
            Ok(response) if response.status().is_server_error() => {
                last_error = Some(format!("crt.sh returned HTTP {}", response.status()));
            }
            Ok(response) => return Err(format!("crt.sh returned HTTP {}", response.status())),
            Err(err) => last_error = Some(format!("request failed: {err}")),
        }

        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
        }
    }

    Err(last_error.unwrap_or_else(|| "crt.sh request failed".to_string()))
}

async fn discover_bufferover(
    domain: &str,
    timeout: Duration,
    include_wildcards: bool,
) -> DiscoveryResult {
    let query_url = format!("https://dns.bufferover.run/dns?q=.{domain}");
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("netutils subdomain")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return DiscoveryResult::error(
                Some(query_url),
                format!("failed to build HTTP client: {err}"),
            )
        }
    };

    let records = match fetch_bufferover_records(&client, &query_url).await {
        Ok(records) => records,
        Err(err) => return DiscoveryResult::error(Some(query_url), err),
    };

    DiscoveryResult {
        subdomains: collect_subdomains(&records, domain, include_wildcards),
        query_url: Some(query_url),
        error: None,
        notes: vec![
            "Bufferover data is derived from historical DNS and certificate datasets, so names can be stale."
                .to_string(),
        ],
    }
}

async fn fetch_bufferover_records(
    client: &reqwest::Client,
    query_url: &str,
) -> Result<Vec<CrtShRecord>, String> {
    let mut last_error = None;
    for attempt in 0..3 {
        match client.get(query_url).send().await {
            Ok(response) if response.status().is_success() => {
                return response
                    .json::<serde_json::Value>()
                    .await
                    .map(|value| extract_bufferover_names(&value))
                    .map_err(|err| format!("failed to parse bufferover JSON: {err}"));
            }
            Ok(response) if response.status().is_server_error() => {
                last_error = Some(format!("bufferover returned HTTP {}", response.status()));
            }
            Ok(response) => return Err(format!("bufferover returned HTTP {}", response.status())),
            Err(err) => last_error = Some(format!("request failed: {err}")),
        }

        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
        }
    }

    Err(last_error.unwrap_or_else(|| "bufferover request failed".to_string()))
}

fn extract_bufferover_names(value: &serde_json::Value) -> Vec<CrtShRecord> {
    let mut records = Vec::new();
    for key in ["FDNS_A", "RDNS"] {
        if let Some(values) = value.get(key).and_then(|value| value.as_array()) {
            for entry in values {
                if let Some(entry) = entry.as_str() {
                    if let Some((_, host)) = entry.split_once(',') {
                        records.push(CrtShRecord {
                            name_value: Some(host.trim().to_string()),
                            common_name: None,
                        });
                    }
                }
            }
        }
    }
    records
}

fn collect_subdomains(
    records: &[CrtShRecord],
    domain: &str,
    include_wildcards: bool,
) -> Vec<String> {
    let mut names = BTreeSet::new();
    for record in records {
        for value in [&record.name_value, &record.common_name]
            .into_iter()
            .flatten()
        {
            for raw_name in value.lines() {
                if let Some(name) = normalize_candidate(raw_name, domain, include_wildcards) {
                    names.insert(name);
                }
            }
        }
    }
    names.into_iter().collect()
}

fn normalize_domain(input: &str) -> Result<String, String> {
    let mut value = input.trim();
    if value.is_empty() {
        return Err("domain is empty".to_string());
    }
    if let Some((_, rest)) = value.split_once("://") {
        value = rest;
    }
    if let Some((before_path, _)) = value.split_once('/') {
        value = before_path;
    }
    if let Some((_, host)) = value.rsplit_once('@') {
        value = host;
    }
    if let Some((host, _)) = value.split_once(':') {
        value = host;
    }

    let domain = value.trim_matches('.').to_ascii_lowercase();
    validate_domain(&domain)?;
    Ok(domain)
}

fn validate_domain(domain: &str) -> Result<(), String> {
    if domain.len() > 253 {
        return Err("domain is longer than 253 characters".to_string());
    }
    if !domain.contains('.') {
        return Err("domain should include at least one dot, for example example.com".to_string());
    }
    for label in domain.split('.') {
        if label.is_empty() {
            return Err("domain contains an empty label".to_string());
        }
        if label.len() > 63 {
            return Err(format!(
                "domain label is longer than 63 characters: {label}"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!("domain label starts or ends with '-': {label}"));
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(format!(
                "domain label contains unsupported characters: {label}"
            ));
        }
    }
    Ok(())
}

fn normalize_candidate(raw_name: &str, domain: &str, include_wildcards: bool) -> Option<String> {
    let name = raw_name.trim().trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty()
        || name.contains(' ')
        || name.contains('@')
        || name.contains(':')
        || name.contains('/')
    {
        return None;
    }

    let stripped = name.strip_prefix("*.").unwrap_or(&name);
    if stripped == domain || !stripped.ends_with(&format!(".{domain}")) {
        return None;
    }
    if validate_domain(stripped).is_err() {
        return None;
    }

    if include_wildcards && name.starts_with("*.") {
        Some(name)
    } else {
        Some(stripped.to_string())
    }
}

fn build_report(
    domain: &str,
    source: Source,
    result: DiscoveryResult,
    include_wildcards: bool,
    max: usize,
) -> SubdomainReport {
    let discovered = result.subdomains.len();
    let subdomains = result.subdomains.into_iter().take(max).collect::<Vec<_>>();
    SubdomainReport {
        domain: domain.to_string(),
        source: source.as_str().to_string(),
        query_url: result.query_url,
        discovered,
        returned: subdomains.len(),
        truncated: discovered > subdomains.len(),
        include_wildcards,
        subdomains,
        error: result.error,
        notes: result.notes,
    }
}

fn error_report(
    input_domain: &str,
    source: Source,
    include_wildcards: bool,
    error: String,
) -> SubdomainReport {
    SubdomainReport {
        domain: input_domain.to_string(),
        source: source.as_str().to_string(),
        query_url: None,
        discovered: 0,
        returned: 0,
        truncated: false,
        include_wildcards,
        subdomains: Vec::new(),
        error: Some(error),
        notes: default_notes(),
    }
}

fn default_notes() -> Vec<String> {
    vec![
        "There is no standard DNS query for all subdomains; this is passive discovery.".to_string(),
        "Passive sources may contain stale names and miss private or non-TLS names.".to_string(),
    ]
}

fn output(report: &SubdomainReport, mode: OutputMode) {
    if mode.is_json() {
        print_json(report);
    } else {
        print_report(report);
    }
}

fn print_report(report: &SubdomainReport) {
    println!();
    println!("🔎 Subdomain Discovery");
    println!("  Domain: {}", report.domain);
    println!("  Source: {}", report.source);
    if let Some(url) = &report.query_url {
        println!("  Query: {url}");
    }
    println!("  Discovered: {}", report.discovered);
    println!("  Returned: {}", report.returned);
    if report.truncated {
        println!("  Truncated: yes");
    }
    if let Some(error) = &report.error {
        println!("  Error: {error}");
    }

    if !report.subdomains.is_empty() {
        println!();
        let rows = report
            .subdomains
            .iter()
            .enumerate()
            .map(|(idx, name)| vec![(idx + 1).to_string(), name.to_string()])
            .collect::<Vec<_>>();
        print_table(&["#", "Subdomain"], &rows);
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
    fn normalizes_domain_from_plain_host() {
        assert_eq!(normalize_domain("Example.COM.").unwrap(), "example.com");
    }

    #[test]
    fn normalizes_domain_from_url() {
        assert_eq!(
            normalize_domain("https://user@example.com:443/path").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn rejects_invalid_domain() {
        assert!(normalize_domain("localhost").is_err());
        assert!(normalize_domain("bad_domain.example.com").is_err());
    }

    #[test]
    fn normalizes_candidates_under_domain() {
        assert_eq!(
            normalize_candidate("API.Example.com.", "example.com", false).as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            normalize_candidate("example.com", "example.com", false),
            None
        );
        assert_eq!(normalize_candidate("other.com", "example.com", false), None);
    }

    #[test]
    fn strips_wildcards_by_default() {
        assert_eq!(
            normalize_candidate("*.api.example.com", "example.com", false).as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            normalize_candidate("*.api.example.com", "example.com", true).as_deref(),
            Some("*.api.example.com")
        );
    }

    #[test]
    fn collects_and_truncates_subdomains() {
        let records = vec![CrtShRecord {
            name_value: Some("b.example.com\na.example.com\n*.a.example.com".to_string()),
            common_name: Some("c.example.com".to_string()),
        }];
        let result = DiscoveryResult {
            subdomains: collect_subdomains(&records, "example.com", false),
            query_url: None,
            error: None,
            notes: default_notes(),
        };
        let report = build_report("example.com", Source::Crtsh, result, false, 2);
        assert_eq!(report.discovered, 3);
        assert!(report.truncated);
        assert_eq!(report.subdomains, vec!["a.example.com", "b.example.com"]);
    }

    #[test]
    fn extracts_bufferover_hosts() {
        let value = serde_json::json!({
            "FDNS_A": ["1.1.1.1,api.example.com", "2.2.2.2,cdn.example.com"],
            "RDNS": ["3.3.3.3,mail.example.com"]
        });
        let records = extract_bufferover_names(&value);
        let subdomains = collect_subdomains(&records, "example.com", false);
        assert_eq!(
            subdomains,
            vec!["api.example.com", "cdn.example.com", "mail.example.com"]
        );
    }
}
