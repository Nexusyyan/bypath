---
name: bypath-agent
description: Operate and customize Bypath per-application transparent proxy rules, upstream proxies, Windows system proxy reading, dry-run rule testing, live connection inspection, and daemon log troubleshooting via the built-in MCP server (`bypath mcp`) or structured JSON CLI (`bypath ctl --json`).
---

# Bypath Agent DIY Automation Skill

Use this skill when the user asks you to customize Bypath rules, change or detect their Windows system proxy, route or bypass specific desktop apps or domains, test why a connection matched a rule, or troubleshoot proxy failures.

## Available Interfaces

1. **Built-in Stdio MCP Server (Preferred when configured)**:
   ```json
   {
     "mcpServers": {
       "bypath": {
         "command": "bypath.exe",
         "args": ["mcp"]
       }
     }
   }
   ```
   Exposed tools:
   - `bypath_status`, `bypath_toggle_engine`, `bypath_get_system_proxy`, `bypath_get_config`
   - `bypath_upsert_proxy`, `bypath_upsert_rule`, `bypath_reorder_rules`, `bypath_delete_rule`
   - `bypath_test_rule`, `bypath_list_connections`, `bypath_list_processes`, `bypath_get_logs`, `bypath_check_updates`

2. **Structured JSON CLI (Zero-setup fallback from any terminal)**:
   ```powershell
   bypath.exe ctl --json status
   bypath.exe ctl --json system-proxy
   bypath.exe ctl --json config get
   bypath.exe ctl --json rule list
   bypath.exe ctl --json rule test --process Antigravity.exe --host aicode.googleapis.com --port 443
   bypath.exe ctl --json logs --level WARN --limit 50
   bypath.exe ctl --json update
   bypath.exe ctl schema
   ```

Both interfaces communicate live with `BypathService` over `\\.\pipe\bypath-v1` (hot-reloading rules without dropping active connections) and fall back cleanly to editing the local config file when the service is stopped.

## Standard DIY Workflows

### 1. Read System Proxy & Configure Upstream
1. Call `bypath_get_system_proxy` (or `bypath ctl --json system-proxy`) to inspect Windows Internet Settings and proxy env vars.
2. Confirm with the user if multiple proxies exist, then call `bypath_upsert_proxy` (or `bypath ctl --json proxy set local_socks --type socks5 --addr 127.0.0.1:7897 --udp true`).
3. Verify connectivity with `bypath ctl --json proxy probe local_socks`.

### 2. Add Per-App Proxy Rule with Domain Exceptions
Remember Bypath's two-phase, top-to-bottom rule evaluation:
- **Phase 1 (L4)**: First matching non-domain rule wins (`process_names`, `path_prefixes`, `path_contains`, `include_children`, `dst_cidrs`, `dst_ports`, `protocols`).
- **Phase 2 (L7 SNI/Host)**: After TLS SNI / HTTP Host recovery, the first matching domain rule overrides Phase 1 (`block`, `direct`, or `proxy`).

Example: Proxy an IDE tree while keeping corporate intranet domains direct:
1. Inspect running processes (`bypath_list_processes` with filter) to get exact `.exe` names and install path substrings.
2. Add the app proxy rule (`bypath_upsert_rule` with `include_children: true`).
3. Add a domain bypass rule (`action: "direct"`, `domains: ["*.corp.internal"]`).
4. **Always dry-run verify** with `bypath_test_rule` for both a public domain (expect `proxy`) and an excluded domain (expect `direct`).

### 3. Troubleshoot Connection Issues
1. Check `bypath_status` (confirm `windivert_ready: true` and `engine_running: true`).
2. Inspect recent connections via `bypath_list_connections` (filter by `process` or `state: "failed"` / `"blocked"`).
3. Inspect recent daemon logs via `bypath_get_logs` (`min_level: "WARN"` or `"INFO"`).
4. Run `bypath_test_rule` to confirm whether an unexpected rule priority shadowed the intended rule; use `bypath_reorder_rules` to fix priority.
