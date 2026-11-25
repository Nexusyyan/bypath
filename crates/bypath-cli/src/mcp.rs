use crate::agent::{parse_proxy_kind, parse_rule_action, AgentClient};
use anyhow::{anyhow, Result};
use bypath_core::config::{RuleAction, RuleConfig};
use bypath_core::rules::RuleTestQuery;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Runs a lightweight zero-external-SDK Stdio JSON-RPC 2.0 MCP server backed by `AgentClient`.
pub async fn run_mcp_stdio(pipe: String, config_path: Option<PathBuf>) -> Result<()> {
    let client = AgentClient::new(pipe, config_path)?;
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(resp) = handle_jsonrpc_line(&client, trimmed).await {
            let mut out = serde_json::to_string(&resp)?;
            out.push('\n');
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// Processes a single JSON-RPC 2.0 line and returns an optional response payload
/// (`None` for notifications that carry no `id`).
pub async fn handle_jsonrpc_line(client: &AgentClient, raw: &str) -> Option<Value> {
    let req: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(err) => {
            return Some(jsonrpc_error(Value::Null, -32700, &format!("parse error: {err}")));
        }
    };

    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

    // Notifications have no `id` and expect no reply per JSON-RPC 2.0.
    let id = id?;

    let response = match method {
        "initialize" => jsonrpc_result(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "bypath-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "ping" => jsonrpc_result(id, json!({})),
        "tools/list" => jsonrpc_result(id, json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match execute_tool(client, name, &args).await {
                Ok(payload) => jsonrpc_result(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                        }],
                        "isError": false
                    }),
                ),
                Err(err) => jsonrpc_result(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": err.to_string()
                        }],
                        "isError": true
                    }),
                ),
            }
        }
        other => jsonrpc_error(id, -32601, &format!("method not found: {other}")),
    };

    Some(response)
}

async fn execute_tool(client: &AgentClient, name: &str, args: &Value) -> Result<Value> {
    match name {
        "bypath_status" => Ok(serde_json::to_value(client.get_status().await?)?),
        "bypath_toggle_engine" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("missing required argument 'action' (start | stop)"))?;
            let msg = match action.to_ascii_lowercase().as_str() {
                "start" => client.start_engine().await?,
                "stop" => client.stop_engine().await?,
                other => return Err(anyhow!("invalid action '{other}': expected start or stop")),
            };
            Ok(json!({ "ok": true, "message": msg }))
        }
        "bypath_get_system_proxy" => Ok(serde_json::to_value(client.get_system_proxy().await?)?),
        "bypath_get_config" => Ok(serde_json::to_value(client.get_config().await?)?),
        "bypath_upsert_proxy" => {
            let pname = req_str(args, "name")?;
            let kind = parse_proxy_kind(args.get("type").and_then(Value::as_str).unwrap_or("socks5"))?;
            let address = req_str(args, "address")?.to_string();
            let username = opt_str(args, "username");
            let password = opt_str(args, "password");
            let udp = args.get("udp").and_then(Value::as_bool).unwrap_or(true);
            let rename_to = opt_str(args, "rename_to");
            let snap = client
                .upsert_proxy(pname, kind, address, username, password, udp, rename_to)
                .await?;
            Ok(json!({
                "ok": true,
                "proxies": snap.config.proxies,
                "config_path": snap.path
            }))
        }
        "bypath_upsert_rule" => {
            let name = req_str(args, "name")?.to_string();
            let action = parse_rule_action(
                args.get("action").and_then(Value::as_str).unwrap_or("proxy"),
            )?;
            let enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let include_children = args
                .get("include_children")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut proxy = opt_str(args, "proxy");
            if action == RuleAction::Proxy && proxy.is_none() {
                let snap = client.get_config().await?;
                proxy = snap.config.proxies.keys().next().cloned();
            }
            if action != RuleAction::Proxy {
                proxy = None;
            }
            let rule = RuleConfig {
                name,
                enabled,
                process_names: str_vec(args, "process_names"),
                path_prefixes: str_vec(args, "path_prefixes"),
                path_contains: str_vec(args, "path_contains"),
                include_children,
                dst_cidrs: str_vec(args, "dst_cidrs"),
                dst_ports: u16_vec(args, "dst_ports"),
                protocols: str_vec(args, "protocols"),
                domains: str_vec(args, "domains"),
                action,
                proxy,
            };
            let index = args
                .get("index")
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            let prepend = args
                .get("prepend")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (idx, snap) = client.upsert_rule(index, rule, prepend).await?;
            Ok(json!({
                "ok": true,
                "index": idx,
                "total_rules": snap.config.rules.len(),
                "rule": snap.config.rules[idx]
            }))
        }
        "bypath_reorder_rules" => {
            let from = args
                .get("from_index")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("missing required integer 'from_index'"))?
                as usize;
            let to = args
                .get("to_index")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("missing required integer 'to_index'"))?
                as usize;
            let snap = client.move_rule(from, to).await?;
            let order: Vec<String> = snap.config.rules.iter().map(|r| r.name.clone()).collect();
            Ok(json!({ "ok": true, "order": order }))
        }
        "bypath_delete_rule" => {
            let selector = req_str(args, "selector")?;
            let (removed, snap) = client.delete_rule(selector).await?;
            Ok(json!({
                "ok": true,
                "removed": removed.name,
                "remaining_rules": snap.config.rules.len()
            }))
        }
        "bypath_test_rule" => {
            let query = RuleTestQuery {
                process_name: opt_str(args, "process_name"),
                exe_path: opt_str(args, "exe_path"),
                host: opt_str(args, "host"),
                dst_ip: opt_str(args, "dst_ip"),
                dst_port: args.get("dst_port").and_then(Value::as_u64).unwrap_or(443) as u16,
                protocol: args
                    .get("protocol")
                    .and_then(Value::as_str)
                    .unwrap_or("tcp")
                    .to_string(),
            };
            Ok(serde_json::to_value(client.test_rule(query).await?)?)
        }
        "bypath_list_connections" => {
            let proc_filter = opt_str(args, "process").map(|s| s.to_ascii_lowercase());
            let state_filter = opt_str(args, "state").map(|s| s.to_ascii_lowercase());
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(500) as usize;
            let conns: Vec<_> = client
                .get_connections()
                .await?
                .into_iter()
                .filter(|c| {
                    if let Some(ref p) = proc_filter {
                        if !c.process_name.to_ascii_lowercase().contains(p) {
                            return false;
                        }
                    }
                    if let Some(ref st) = state_filter {
                        let actual = format!("{:?}", c.state).to_ascii_lowercase();
                        if actual != *st {
                            return false;
                        }
                    }
                    true
                })
                .take(limit)
                .collect();
            Ok(serde_json::to_value(conns)?)
        }
        "bypath_list_processes" => {
            let filter = opt_str(args, "filter");
            Ok(serde_json::to_value(client.list_processes(filter).await?)?)
        }
        "bypath_get_logs" => {
            let min_level = opt_str(args, "min_level");
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            Ok(serde_json::to_value(client.get_logs(min_level, limit).await?)?)
        }
        "bypath_check_updates" => {
            let proxy = opt_str(args, "proxy");
            Ok(serde_json::to_value(
                client.check_for_updates(proxy.as_deref()).await?,
            )?)
        }
        other => Err(anyhow!("unknown tool '{other}'")),
    }
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("missing required string argument '{key}'"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn str_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn u16_vec(args: &Value, key: &str) -> Vec<u16> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_u64)
                .filter_map(|v| u16::try_from(v).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "bypath_status",
            "description": "Returns live Bypath daemon status, WinDivert readiness, config path, active rule counts, and throughput stats.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "bypath_toggle_engine",
            "description": "Starts or stops transparent packet interception in Bypath.",
            "inputSchema": {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["start", "stop"] }
                }
            }
        },
        {
            "name": "bypath_get_system_proxy",
            "description": "Reads the current Windows Internet Settings system proxy and proxy environment variables without modifying Bypath config.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "bypath_get_config",
            "description": "Reads the full Bypath configuration snapshot (general preferences, upstream proxies, and ordered rules).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "bypath_upsert_proxy",
            "description": "Creates or updates an upstream SOCKS5/HTTP proxy profile and hot-reloads the running engine. Supports safe renaming with automatic rule reference updates.",
            "inputSchema": {
                "type": "object",
                "required": ["name", "address"],
                "properties": {
                    "name": { "type": "string", "description": "Profile key (e.g. local_socks)" },
                    "type": { "type": "string", "enum": ["socks5", "http"], "default": "socks5" },
                    "address": { "type": "string", "description": "Host and port (e.g. 127.0.0.1:7897)" },
                    "username": { "type": "string" },
                    "password": { "type": "string" },
                    "udp": { "type": "boolean", "default": true },
                    "rename_to": { "type": "string", "description": "Optional new profile key" }
                }
            }
        },
        {
            "name": "bypath_upsert_rule",
            "description": "Creates or updates a routing rule (process tree, path, domain SNI/Host, CIDR, port, protocol) and hot-reloads immediately.",
            "inputSchema": {
                "type": "object",
                "required": ["name", "action"],
                "properties": {
                    "index": { "type": "integer", "description": "Optional 0-based index of existing rule to edit in place" },
                    "prepend": { "type": "boolean", "description": "Insert at top priority (index 0) when creating a new rule" },
                    "name": { "type": "string" },
                    "enabled": { "type": "boolean", "default": true },
                    "action": { "type": "string", "enum": ["proxy", "direct", "block"] },
                    "proxy": { "type": "string", "description": "Upstream proxy profile name when action=proxy" },
                    "process_names": { "type": "array", "items": { "type": "string" } },
                    "path_prefixes": { "type": "array", "items": { "type": "string" } },
                    "path_contains": { "type": "array", "items": { "type": "string" } },
                    "include_children": { "type": "boolean", "default": true },
                    "domains": { "type": "array", "items": { "type": "string" }, "description": "Exact, *.suffix, .suffix, or glob patterns" },
                    "dst_cidrs": { "type": "array", "items": { "type": "string" } },
                    "dst_ports": { "type": "array", "items": { "type": "integer" } },
                    "protocols": { "type": "array", "items": { "type": "string", "enum": ["tcp", "udp"] } }
                }
            }
        },
        {
            "name": "bypath_reorder_rules",
            "description": "Moves a rule from from_index to to_index (0-based priority order, first match wins) and hot-reloads.",
            "inputSchema": {
                "type": "object",
                "required": ["from_index", "to_index"],
                "properties": {
                    "from_index": { "type": "integer" },
                    "to_index": { "type": "integer" }
                }
            }
        },
        {
            "name": "bypath_delete_rule",
            "description": "Deletes a rule by 0-based index or exact rule name and hot-reloads.",
            "inputSchema": {
                "type": "object",
                "required": ["selector"],
                "properties": {
                    "selector": { "type": "string", "description": "0-based index string or rule name" }
                }
            }
        },
        {
            "name": "bypath_test_rule",
            "description": "Dry-runs Phase 1 (L4/process) and Phase 2 (L7 SNI/Host) routing evaluation for a hypothetical connection.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "process_name": { "type": "string" },
                    "exe_path": { "type": "string" },
                    "host": { "type": "string", "description": "Target hostname / TLS SNI" },
                    "dst_ip": { "type": "string" },
                    "dst_port": { "type": "integer", "default": 443 },
                    "protocol": { "type": "string", "enum": ["tcp", "udp"], "default": "tcp" }
                }
            }
        },
        {
            "name": "bypath_list_connections",
            "description": "Lists live and recent connections tracked by Bypath.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "process": { "type": "string", "description": "Optional process name substring filter" },
                    "state": { "type": "string", "enum": ["active", "closed", "blocked", "failed"] },
                    "limit": { "type": "integer", "default": 100 }
                }
            }
        },
        {
            "name": "bypath_list_processes",
            "description": "Lists running OS processes with PID, PPID, executable path, and currently matched Bypath rule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filter": { "type": "string" }
                }
            }
        },
        {
            "name": "bypath_get_logs",
            "description": "Retrieves recent structured diagnostic logs from the Bypath daemon ring buffer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "min_level": { "type": "string", "enum": ["INFO", "WARN", "ERROR"] },
                    "limit": { "type": "integer", "default": 100 }
                }
            }
        },
        {
            "name": "bypath_check_updates",
            "description": "Checks GitHub Releases for a newer Bypath version using configured upstream proxies or the Windows system proxy before falling back to direct.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "proxy": { "type": "string", "description": "Optional explicit proxy URI override (e.g. socks5://127.0.0.1:10808)" }
                }
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mcp_initialize_list_and_rule_workflow() {
        let tmp = std::env::temp_dir().join(format!("bypath-mcp-test-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let client = AgentClient::new(r"\\.\pipe\bypath-nonexistent-test", Some(tmp.clone())).unwrap();

        let init = handle_jsonrpc_line(
            &client,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .await
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "bypath-mcp");

        let list = handle_jsonrpc_line(
            &client,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        )
        .await
        .unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 13);

        let add = handle_jsonrpc_line(
            &client,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"bypath_upsert_rule","arguments":{"name":"Block Ads","action":"block","domains":["*.ads.example"]}}}"#,
        )
        .await
        .unwrap();
        assert_eq!(add["result"]["isError"], false);

        let test_res = handle_jsonrpc_line(
            &client,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"bypath_test_rule","arguments":{"process_name":"Antigravity.exe","host":"cdn.ads.example","dst_port":443}}}"#,
        )
        .await
        .unwrap();
        assert_eq!(test_res["result"]["isError"], false);
        let text = test_res["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"final_action\": \"block\""));

        let _ = std::fs::remove_file(&tmp);
    }
}
