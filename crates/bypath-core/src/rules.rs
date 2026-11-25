use crate::capture::{FiveTuple, TransportProtocol};
use crate::config::{Config, RuleAction, RuleConfig};
use crate::process::{ProcessInfo, ProcessTracker};
use ipnet::IpNet;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::RwLock;

/// Final L4 routing verdict for a flow before application-layer payload sniffing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    Direct { reason: &'static str },
    /// Flow matches a process-scoped domain rule (no process-level proxy rule matched).
    /// Intercept TCP to sniff TLS SNI / HTTP Host, then either Block or relay directly.
    InspectDirect { rule_name: String },
    Block { rule_name: String },
    Proxy { rule_name: String, proxy_name: String },
}

/// Application-layer routing override decided after recovering TLS SNI or HTTP Host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainDecision {
    Direct { rule_name: String },
    Block { rule_name: String },
    Proxy { rule_name: String, proxy_name: String },
}

#[derive(Debug, Clone)]
enum DomainPattern {
    /// Exact hostname match (`api.example.com`).
    Exact(String),
    /// Matches apex (`example.com`) and any subdomain (`a.b.example.com`).
    Suffix(String),
    /// General wildcard pattern with `*` and `?` (`*telemetry*.example.com`).
    Glob(String),
}

impl DomainPattern {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim().trim_end_matches('.').to_ascii_lowercase();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(rest) = trimmed.strip_prefix("*.") {
            if !rest.is_empty() && !rest.contains('*') && !rest.contains('?') {
                return Some(Self::Suffix(rest.to_string()));
            }
        }
        if let Some(rest) = trimmed.strip_prefix('.') {
            if !rest.is_empty() && !rest.contains('*') && !rest.contains('?') {
                return Some(Self::Suffix(rest.to_string()));
            }
        }
        if trimmed.contains('*') || trimmed.contains('?') {
            return Some(Self::Glob(trimmed));
        }
        Some(Self::Exact(trimmed))
    }

    fn matches(&self, normalized_host: &str) -> bool {
        match self {
            Self::Exact(d) => normalized_host == d,
            Self::Suffix(suffix) => {
                normalized_host == suffix
                    || (normalized_host.len() > suffix.len()
                        && normalized_host.ends_with(suffix)
                        && normalized_host.as_bytes()[normalized_host.len() - suffix.len() - 1]
                            == b'.')
            }
            Self::Glob(pat) => glob_matches(pat.as_bytes(), normalized_host.as_bytes()),
        }
    }
}

/// Linear-time wildcard glob matching (`*` matches 0+ bytes, `?` matches 1 byte).
fn glob_matches(pattern: &[u8], text: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = Some(pi);
            pi += 1;
            star_ti = ti;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

struct CompiledRule {
    name: String,
    process_names_lower: Vec<String>,
    path_prefixes_lower: Vec<String>,
    path_contains_lower: Vec<String>,
    include_children: bool,
    dst_cidrs: Vec<IpNet>,
    dst_ports: Vec<u16>,
    protocols: Vec<TransportProtocol>,
    domains: Vec<DomainPattern>,
    action: RuleAction,
    proxy_name: Option<String>,
}

impl CompiledRule {
    fn has_proc_criteria(&self) -> bool {
        !self.process_names_lower.is_empty()
            || !self.path_prefixes_lower.is_empty()
            || !self.path_contains_lower.is_empty()
    }
}

/// Evaluates network flows against built-in safety exemptions, process rules, and domain rules.
pub struct RuleEngine {
    bypass_lan: bool,
    exempt_pids: RwLock<HashSet<u32>>,
    rules: Vec<CompiledRule>,
    lan_nets: Vec<IpNet>,
}

impl RuleEngine {
    pub fn from_config(config: &Config) -> anyhow::Result<Self> {
        let mut rules = Vec::new();
        for rc in &config.rules {
            if !rc.enabled {
                continue;
            }
            rules.push(compile_rule(rc)?);
        }

        let mut exempt = HashSet::new();
        exempt.insert(std::process::id());

        let lan_nets = [
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "169.254.0.0/16",
            "100.64.0.0/10",
            "fc00::/7",
            "fe80::/10",
        ]
        .iter()
        .map(|s| s.parse::<IpNet>().expect("valid built-in CIDR"))
        .collect();

        Ok(Self {
            bypass_lan: config.general.bypass_lan,
            exempt_pids: RwLock::new(exempt),
            rules,
            lan_nets,
        })
    }

    /// Registers an additional exempt PID (for example, the local upstream proxy daemon)
    /// so its outbound connections are never re-intercepted.
    pub fn add_exempt_pid(&self, pid: u32) {
        if pid != 0 {
            if let Ok(mut guard) = self.exempt_pids.write() {
                guard.insert(pid);
            }
        }
    }

    /// Evaluates L4 routing for a given `(pid, tuple)` before domain sniffing.
    pub fn evaluate(
        &self,
        pid: Option<u32>,
        tuple: &FiveTuple,
        tracker: &dyn ProcessTracker,
    ) -> RouteDecision {
        // 1. Always bypass loopback, multicast, unspecified, and broadcast addresses.
        if is_always_direct_ip(tuple.dst_ip) {
            return RouteDecision::Direct {
                reason: "loopback_or_special",
            };
        }

        // 2. Always bypass self and upstream proxy PIDs to guarantee anti-loop safety.
        if let Some(p) = pid {
            if let Ok(guard) = self.exempt_pids.read() {
                if guard.contains(&p) {
                    return RouteDecision::Direct {
                        reason: "exempt_pid",
                    };
                }
            }
        }

        // 3. Optional private/LAN bypass.
        if self.bypass_lan && self.lan_nets.iter().any(|net| net.contains(&tuple.dst_ip)) {
            return RouteDecision::Direct { reason: "lan_cidr" };
        }

        // 4. Resolve process + ancestor chain once per evaluation.
        let chain: Vec<ProcessInfo> = match pid {
            Some(p) if p != 0 => tracker.ancestor_chain(p),
            _ => Vec::new(),
        };

        // 5. Walk user rules in declaration order.
        // Rules with non-empty `domains` are evaluated in Phase 2 after SNI/Host sniffing;
        // if a process-scoped domain rule matches a non-proxied TCP flow, we capture it in
        // `InspectDirect` mode so non-matching domains still pass directly untouched.
        let mut inspect_direct_rule: Option<&str> = None;

        for rule in &self.rules {
            if !rule.domains.is_empty() {
                if inspect_direct_rule.is_none()
                    && tuple.protocol == TransportProtocol::Tcp
                    && rule.has_proc_criteria()
                    && rule_matches_l4_and_proc(rule, tuple, &chain)
                {
                    inspect_direct_rule = Some(&rule.name);
                }
                continue;
            }

            if rule_matches_l4_and_proc(rule, tuple, &chain) {
                return match rule.action {
                    RuleAction::Proxy => RouteDecision::Proxy {
                        rule_name: rule.name.clone(),
                        proxy_name: rule.proxy_name.clone().unwrap_or_default(),
                    },
                    RuleAction::Direct => RouteDecision::Direct {
                        reason: "matched_direct_rule",
                    },
                    RuleAction::Block => RouteDecision::Block {
                        rule_name: rule.name.clone(),
                    },
                };
            }
        }

        if let Some(rule_name) = inspect_direct_rule {
            return RouteDecision::InspectDirect {
                rule_name: rule_name.to_string(),
            };
        }

        RouteDecision::Direct {
            reason: "default_direct",
        }
    }

    /// Evaluates domain-bearing rules (Phase 2) once TLS SNI or HTTP Host is recovered.
    /// Returns `Some(DomainDecision)` on the first matching domain rule, or `None` to keep
    /// the Phase 1 routing decision.
    pub fn evaluate_domain(
        &self,
        pid: Option<u32>,
        tuple: &FiveTuple,
        host: &str,
        tracker: &dyn ProcessTracker,
    ) -> Option<DomainDecision> {
        let norm_host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if norm_host.is_empty() {
            return None;
        }

        let has_any_domain_rule = self.rules.iter().any(|r| !r.domains.is_empty());
        if !has_any_domain_rule {
            return None;
        }

        let chain: Vec<ProcessInfo> = match pid {
            Some(p) if p != 0 => tracker.ancestor_chain(p),
            _ => Vec::new(),
        };

        for rule in &self.rules {
            if rule.domains.is_empty() {
                continue;
            }
            if !rule_matches_l4_and_proc(rule, tuple, &chain) {
                continue;
            }
            if rule.domains.iter().any(|pat| pat.matches(&norm_host)) {
                return Some(match rule.action {
                    RuleAction::Block => DomainDecision::Block {
                        rule_name: rule.name.clone(),
                    },
                    RuleAction::Direct => DomainDecision::Direct {
                        rule_name: rule.name.clone(),
                    },
                    RuleAction::Proxy => DomainDecision::Proxy {
                        rule_name: rule.name.clone(),
                        proxy_name: rule.proxy_name.clone().unwrap_or_default(),
                    },
                });
            }
        }

        None
    }

    /// Returns true if any enabled domain-blocking rule applies to the given process/flow,
    /// signalling that encrypted QUIC (`UDP/443`) should be dropped so the client falls back
    /// to inspectable TLS/TCP.
    pub fn has_domain_block_rules_for_flow(
        &self,
        pid: Option<u32>,
        tuple: &FiveTuple,
        tracker: &dyn ProcessTracker,
    ) -> bool {
        let any_block = self
            .rules
            .iter()
            .any(|r| !r.domains.is_empty() && r.action == RuleAction::Block);
        if !any_block {
            return false;
        }

        let chain: Vec<ProcessInfo> = match pid {
            Some(p) if p != 0 => tracker.ancestor_chain(p),
            _ => Vec::new(),
        };

        self.rules.iter().any(|rule| {
            !rule.domains.is_empty()
                && rule.action == RuleAction::Block
                && rule_matches_proc_and_target(rule, tuple, &chain)
        })
    }
}

fn compile_rule(rc: &RuleConfig) -> anyhow::Result<CompiledRule> {
    let mut dst_cidrs = Vec::with_capacity(rc.dst_cidrs.len());
    for cidr in &rc.dst_cidrs {
        dst_cidrs.push(cidr.parse::<IpNet>()?);
    }

    let mut protocols = Vec::with_capacity(rc.protocols.len());
    for p in &rc.protocols {
        match p.trim().to_ascii_lowercase().as_str() {
            "tcp" => protocols.push(TransportProtocol::Tcp),
            "udp" => protocols.push(TransportProtocol::Udp),
            other => anyhow::bail!("unsupported protocol '{other}' in rule '{}'", rc.name),
        }
    }

    let domains = rc
        .domains
        .iter()
        .filter_map(|d| DomainPattern::parse(d))
        .collect();

    Ok(CompiledRule {
        name: rc.name.clone(),
        process_names_lower: rc
            .process_names
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect(),
        path_prefixes_lower: rc
            .path_prefixes
            .iter()
            .map(|s| normalize_path(s))
            .collect(),
        path_contains_lower: rc
            .path_contains
            .iter()
            .map(|s| normalize_path(s))
            .collect(),
        include_children: rc.include_children,
        dst_cidrs,
        dst_ports: rc.dst_ports.clone(),
        protocols,
        domains,
        action: rc.action,
        proxy_name: rc.proxy.clone(),
    })
}

fn normalize_path(p: &str) -> String {
    p.replace('/', "\\").to_ascii_lowercase()
}

fn is_always_direct_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast()
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_multicast(),
    }
}

fn rule_matches_l4_and_proc(rule: &CompiledRule, tuple: &FiveTuple, chain: &[ProcessInfo]) -> bool {
    if !rule.protocols.is_empty() && !rule.protocols.contains(&tuple.protocol) {
        return false;
    }
    rule_matches_proc_and_target(rule, tuple, chain)
}

fn rule_matches_proc_and_target(
    rule: &CompiledRule,
    tuple: &FiveTuple,
    chain: &[ProcessInfo],
) -> bool {
    if !rule.dst_ports.is_empty() && !rule.dst_ports.contains(&tuple.dst_port) {
        return false;
    }
    if !rule.dst_cidrs.is_empty() && !rule.dst_cidrs.iter().any(|net| net.contains(&tuple.dst_ip)) {
        return false;
    }

    if !rule.has_proc_criteria() {
        return true;
    }
    if chain.is_empty() {
        return false;
    }

    let candidates: &[ProcessInfo] = if rule.include_children {
        chain
    } else {
        &chain[..1]
    };

    candidates.iter().any(|proc_info| {
        let name_lower = proc_info.name.to_ascii_lowercase();
        if rule.process_names_lower.iter().any(|n| n == &name_lower) {
            return true;
        }
        if let Some(ref exe) = proc_info.exe_path {
            let norm = normalize_path(exe);
            if rule
                .path_prefixes_lower
                .iter()
                .any(|pref| norm.starts_with(pref))
            {
                return true;
            }
            if rule
                .path_contains_lower
                .iter()
                .any(|sub| norm.contains(sub))
            {
                return true;
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{builtin_preset_rule, GeneralConfig, ProxyConfig, ProxyKind};
    use crate::process::MockProcessTracker;
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    fn sample_config() -> Config {
        let mut proxies = HashMap::new();
        proxies.insert(
            "local".to_string(),
            ProxyConfig {
                kind: ProxyKind::Socks5,
                address: "127.0.0.1:7890".to_string(),
                username: None,
                password: None,
                udp: true,
            },
        );
        Config {
            general: GeneralConfig::default(),
            proxies,
            rules: vec![builtin_preset_rule("antigravity", "local").unwrap()],
        }
    }

    fn tcp_tuple(dst: [u8; 4], port: u16) -> FiveTuple {
        FiveTuple {
            protocol: TransportProtocol::Tcp,
            src_ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
            src_port: 55000,
            dst_ip: IpAddr::V4(Ipv4Addr::from(dst)),
            dst_port: port,
        }
    }

    #[test]
    fn grandchild_process_inherits_antigravity_rule() {
        let engine = RuleEngine::from_config(&sample_config()).unwrap();
        let tracker = MockProcessTracker::with_processes([
            ProcessInfo {
                pid: 1000,
                parent_pid: 4,
                name: "Antigravity.exe".to_string(),
                exe_path: Some(r"C:\Users\dev\AppData\Local\Antigravity\Antigravity.exe".into()),
            },
            ProcessInfo {
                pid: 1001,
                parent_pid: 1000,
                name: "language_server_windows_x64.exe".to_string(),
                exe_path: Some(
                    r"C:\Users\dev\AppData\Local\Antigravity\bin\language_server_windows_x64.exe"
                        .into(),
                ),
            },
            ProcessInfo {
                pid: 1002,
                parent_pid: 1001,
                name: "node.exe".to_string(),
                exe_path: Some(r"C:\Program Files\nodejs\node.exe".into()),
            },
            ProcessInfo {
                pid: 2000,
                parent_pid: 4,
                name: "chrome.exe".to_string(),
                exe_path: Some(r"C:\Program Files\Google\Chrome\chrome.exe".into()),
            },
        ]);

        // Grandchild node.exe (MCP server) inherits Antigravity proxy rule
        let decision = engine.evaluate(Some(1002), &tcp_tuple([142, 250, 190, 78], 443), &tracker);
        assert!(matches!(
            decision,
            RouteDecision::Proxy { ref proxy_name, .. } if proxy_name == "local"
        ));

        // Unrelated chrome.exe goes direct
        let other = engine.evaluate(Some(2000), &tcp_tuple([142, 250, 190, 78], 443), &tracker);
        assert_eq!(
            other,
            RouteDecision::Direct {
                reason: "default_direct"
            }
        );
    }

    #[test]
    fn loopback_lan_and_exempt_pid_always_bypass() {
        let engine = RuleEngine::from_config(&sample_config()).unwrap();
        engine.add_exempt_pid(1000);
        let tracker = MockProcessTracker::with_processes([ProcessInfo {
            pid: 1000,
            parent_pid: 4,
            name: "Antigravity.exe".to_string(),
            exe_path: Some(r"C:\Antigravity\Antigravity.exe".into()),
        }]);

        // Loopback always direct
        assert_eq!(
            engine.evaluate(Some(9999), &tcp_tuple([127, 0, 0, 1], 45678), &tracker),
            RouteDecision::Direct {
                reason: "loopback_or_special"
            }
        );

        // Exempt PID always direct even though its process name matches a proxy rule
        assert_eq!(
            engine.evaluate(Some(1000), &tcp_tuple([8, 8, 8, 8], 443), &tracker),
            RouteDecision::Direct {
                reason: "exempt_pid"
            }
        );

        // LAN bypass
        assert_eq!(
            engine.evaluate(Some(9999), &tcp_tuple([192, 168, 1, 1], 80), &tracker),
            RouteDecision::Direct { reason: "lan_cidr" }
        );
    }

    #[test]
    fn domain_pattern_matching_supports_exact_suffix_and_glob() {
        let exact = DomainPattern::parse("Play.GoogleApis.com.").unwrap();
        assert!(exact.matches("play.googleapis.com"));
        assert!(!exact.matches("sub.play.googleapis.com"));

        let suffix = DomainPattern::parse("*.doubleclick.net").unwrap();
        assert!(suffix.matches("doubleclick.net"));
        assert!(suffix.matches("ad.doubleclick.net"));
        assert!(suffix.matches("a.b.doubleclick.net"));
        assert!(!suffix.matches("notdoubleclick.net"));

        let dot_suffix = DomainPattern::parse(".analytics.example.com").unwrap();
        assert!(dot_suffix.matches("analytics.example.com"));
        assert!(dot_suffix.matches("v2.analytics.example.com"));
        assert!(!dot_suffix.matches("badanalytics.example.com"));

        let glob = DomainPattern::parse("*telemetry*.example.com").unwrap();
        assert!(glob.matches("telemetry.example.com"));
        assert!(glob.matches("us-telemetry-v2.example.com"));
        assert!(!glob.matches("api.example.com"));
    }

    #[test]
    fn domain_rules_override_proxied_flows_and_inspect_scoped_processes() {
        let mut cfg = sample_config();
        cfg.rules.push(RuleConfig {
            name: "Block Telemetry".into(),
            enabled: true,
            process_names: vec![],
            path_prefixes: vec![],
            path_contains: vec![],
            include_children: true,
            dst_cidrs: vec![],
            dst_ports: vec![],
            protocols: vec![],
            domains: vec!["*.doubleclick.net".into(), "play.googleapis.com".into()],
            action: RuleAction::Block,
            proxy: None,
        });
        cfg.rules.push(RuleConfig {
            name: "Block Chrome Ads".into(),
            enabled: true,
            process_names: vec!["chrome.exe".into()],
            path_prefixes: vec![],
            path_contains: vec![],
            include_children: true,
            dst_cidrs: vec![],
            dst_ports: vec![],
            protocols: vec![],
            domains: vec!["*.ads.example".into()],
            action: RuleAction::Block,
            proxy: None,
        });

        let engine = RuleEngine::from_config(&cfg).unwrap();
        let tracker = MockProcessTracker::with_processes([
            ProcessInfo {
                pid: 1000,
                parent_pid: 4,
                name: "Antigravity.exe".into(),
                exe_path: Some(r"C:\Antigravity\Antigravity.exe".into()),
            },
            ProcessInfo {
                pid: 2000,
                parent_pid: 4,
                name: "chrome.exe".into(),
                exe_path: Some(r"C:\Chrome\chrome.exe".into()),
            },
            ProcessInfo {
                pid: 3000,
                parent_pid: 4,
                name: "notepad.exe".into(),
                exe_path: Some(r"C:\Windows\notepad.exe".into()),
            },
        ]);

        let tuple = tcp_tuple([142, 250, 190, 78], 443);

        // Antigravity L4 stays Proxy; Phase 2 blocks matching domain
        assert!(matches!(
            engine.evaluate(Some(1000), &tuple, &tracker),
            RouteDecision::Proxy { .. }
        ));
        assert_eq!(
            engine.evaluate_domain(Some(1000), &tuple, "play.googleapis.com", &tracker),
            Some(DomainDecision::Block {
                rule_name: "Block Telemetry".into()
            })
        );
        assert_eq!(
            engine.evaluate_domain(Some(1000), &tuple, "aicode.googleapis.com", &tracker),
            None
        );

        // Chrome has a process-scoped domain block rule -> InspectDirect at L4
        assert_eq!(
            engine.evaluate(Some(2000), &tuple, &tracker),
            RouteDecision::InspectDirect {
                rule_name: "Block Chrome Ads".into()
            }
        );
        assert_eq!(
            engine.evaluate_domain(Some(2000), &tuple, "cdn.ads.example", &tracker),
            Some(DomainDecision::Block {
                rule_name: "Block Chrome Ads".into()
            })
        );

        // Unrelated notepad.exe is NOT intercepted by domain-only rules (zero disruption)
        assert_eq!(
            engine.evaluate(Some(3000), &tuple, &tracker),
            RouteDecision::Direct {
                reason: "default_direct"
            }
        );
    }
}
