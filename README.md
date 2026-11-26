<p align="center">
  <img src="assets/bypath-banner.svg" alt="Bypath" width="560" />
</p>

<p align="center">
  <strong>Open-source per-application transparent proxy &amp; domain control in Rust.</strong><br />
  Route selected apps and their full child process trees through SOCKS5 or HTTP proxies — with zero TUN adapters, zero routing table changes, one-click novice UX, and native AI Agent / MCP automation.
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="DESIGN.md">Design System</a> · <a href="skills/bypath-agent/SKILL.md">Agent Skill</a>
</p>

---

Bypath transparently intercepts TCP and UDP traffic of selected desktop applications—including their entire spawned child process tree (such as Electron clients + native Go/Rust/C++ language servers and MCP helpers)—and bridges it to your upstream SOCKS5 or HTTP proxy without creating a global TUN device or hijacking `0.0.0.0/0`.

## Key Features

- **Out-of-the-box GitHub Release experience**: Download the recommended `bypath-<tag>-windows-x64.zip` (bundled with official SHA-256 & Authenticode verified `WinDivert.dll` + `WinDivert64.sys` and full LGPLv3 notices, zero sample tools), launch `bypath-gui.exe`, adjust your proxy port if needed, authorize `BypathService` once, and toggle proxying in one click. A clean `-nodriver.zip` variant is also published for restricted AV environments.
- **Zero global routing conflicts**: Works alongside corporate VPNs, WSL2, Docker Desktop, Hyper-V, and LAN devices without touching the OS routing table.
- **Full process tree & directory matching**: Automatically inherits spawned child processes (`language_server.exe`, MCP servers, CLI helpers), accelerated by O(1) per-PID `NtQueryInformationProcess` resolution, WinDivert `SOCKET` cache pre-warming, and dynamic upstream-proxy anti-loop PID refresh.
- **Post-quantum TLS SNI sniffing & two-phase domain rules**: Accumulates fragmented multi-segment TLS ClientHellos (such as Chrome/Electron/Go `X25519MLKEM768`) alongside HTTP `Host` and QUIC Initial detection, enabling clean remote DNS resolution (`ATYP=0x03` / `CONNECT host:port`) plus exact, wildcard (`*.example.com`), and glob domain `Block` / `Direct` / `Proxy` overrides.
- **Desktop ergonomics & observability**: System tray minimization with a custom CSS-styled frameless bilingual tray popover (live throughput, one-click toggle, tab jumps, and synced Light/Dark/System + ZH/EN switches), Win32 single-instance guard, Light / Pure Charcoal Dark / System themes, proxy-aware GitHub Release update checker, custom rounded controls, persistent tab state, keyboard shortcuts (`Ctrl+K`, `Ctrl+1..4`, `Space`, `Esc`), 60-second throughput sparklines, process/domain summary aggregation, live daemon log viewer, rule priority reordering, dry-run rule match tester, and TOML/CSV import/export + Portable Mode (`portable.json`).
- **AI Agent & DIY first-class interfaces**: Built-in stdio MCP Server (`bypath mcp`) and structured JSON CLI (`bypath ctl --json`) let Antigravity, Claude Code, or Cursor inspect Windows system proxies, edit/reorder rules, run dry-run rule tests, check for updates, and analyze live logs automatically.

## Quick Start (Windows 10/11 x64)

### Option A — Download from GitHub Releases (Recommended)

1. Download and extract **`bypath-v0.4.1-windows-x64.zip`** from [GitHub Releases](https://github.com/Nexusyyan/bypath/releases).
2. Double-click **`bypath-gui.exe`**.
3. If your local proxy uses a port other than `127.0.0.1:10808` (for example Clash `127.0.0.1:7897`), click **Edit Proxy** (or **Read System Proxy** inside **Proxies**), save, and click **Authorize & Start Service** once.
4. Toggle presets (e.g. **Antigravity**, **Cursor**, **VS Code**, **Git**) and click **Start Proxying**. Closing the window minimizes Bypath quietly to the system tray.

> **Portable Mode**: Drop an empty `portable.json` (`{"portable":true}`) next to `bypath-gui.exe` to keep `config.toml` and `logs/` inside `./bypath-data/`.

### Option B — Build from Source

```powershell
# 1. Fetch official signed WinDivert 2.2.2-A runtime (pass -Proxy http://127.0.0.1:7897 if needed)
.\scripts\fetch-windivert.ps1

# 2. Build CLI + Background Service Daemon
cargo build --workspace --release

# 3. Build Tauri v2 Desktop GUI (embeds production frontend assets)
npm --prefix apps/desktop install
npm --prefix apps/desktop run tauri:build
```

### AI Agent / MCP DIY Automation

Connect Antigravity, Claude Code, or Cursor directly to Bypath's built-in stdio MCP server:

```json
{
  "mcpServers": {
    "bypath": {
      "command": "C:\\path\\to\\bypath.exe",
      "args": ["mcp"]
    }
  }
}
```

Or drive everything with structured JSON from any terminal:

```powershell
.\bypath.exe ctl --json status
.\bypath.exe ctl --json system-proxy
.\bypath.exe ctl --json rule test --process Antigravity.exe --host aicode.googleapis.com --port 443
.\bypath.exe ctl --json logs --level WARN --limit 50
.\bypath.exe ctl --json update
.\bypath.exe ctl schema
```

See [skills/bypath-agent/SKILL.md](skills/bypath-agent/SKILL.md) for complete Agent workflows.

## Architecture

- `crates/bypath-core`: Capture traits, fast process-tree tracker, two-phase rule engine with dry-run explainer, multi-segment TLS/HTTP/QUIC sniffer, `smoltcp` userspace TCP/IP netstack, `TelemetryHub` + live `LogRingBuffer`, real handshake proxy probe, and on-demand Windows system proxy reader.
- `crates/bypath-windivert`: Runtime-loaded `WinDivert.dll` + `SOCKET`/`FLOW`/`NETWORK` layers + IPHelper fallback with structured error diagnostics.
- `crates/bypath-ipc`: NDJSON RPC + event/log streaming over Windows Named Pipe (`\\.\pipe\bypath-v1`, local-interactive SDDL ACLs, remote-client rejection).
- `crates/bypath-daemon`: Shared `DaemonController`, Windows Service (`BypathService`), rolling log writer, and `bypath-daemon.exe`.
- `crates/bypath-cli`: Foreground runner (`run`), diagnostics (`doctor`), process list (`ps`), service manager (`service`), full JSON automation CLI (`ctl --json`), and native stdio MCP server (`mcp`).
- `apps/desktop`: Minimalist Tauri v2 + React + TypeScript desktop app (`bypath-gui.exe`) with self-hosted service fallback, Win32 single-instance guard, bilingual dynamic system tray, Light/Dark/System theme, and full rule/proxy/log observability UI.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for WinDivert LGPLv3 dynamic-linking compliance details.
