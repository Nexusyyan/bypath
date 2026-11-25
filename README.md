<p align="center">
  <img src="assets/bypath-banner.svg" alt="Bypath" width="560" />
</p>

<p align="center">
  <strong>Open-source per-application transparent proxy &amp; domain control in Rust.</strong><br />
  Route selected apps and their full child process trees through SOCKS5 or HTTP proxies — with zero TUN adapters and zero routing table changes.
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="DESIGN.md">Design System</a>
</p>

---

Bypath transparently intercepts TCP and UDP traffic of selected desktop applications—including their entire spawned child process tree (such as Electron clients + native Go/Rust/C++ language servers and MCP helpers)—and bridges it to your upstream SOCKS5 or HTTP proxy without creating a global TUN device or hijacking `0.0.0.0/0`.

## Key Features

- **Zero global routing conflicts**: Works alongside corporate VPNs, WSL2, Docker Desktop, Hyper-V, and LAN devices without touching the OS routing table.
- **Full process tree & directory matching**: Automatically inherits spawned child processes (`language_server.exe`, MCP servers, CLI helpers), accelerated by O(1) per-PID `NtQueryInformationProcess` resolution and WinDivert `SOCKET` cache pre-warming.
- **DNS poisoning immunity & SNI domain blocking**: Built-in TLS SNI, HTTP `Host`, and QUIC Initial inspection recovers true hostnames for remote upstream resolution (`ATYP=0x03` / `CONNECT host:port`) and enforces exact, wildcard (`*.example.com`), and glob domain blocking rules before dialing upstream.
- **QUIC / UDP governance**: Supports RFC 1928 SOCKS5 `UDP ASSOCIATE`, plus automatic QUIC (`UDP/443`) fallback dropping when an upstream proxy is TCP-only or when domain-blocking rules require TLS SNI inspection.
- **Live connection monitor with process tabs, sorting & pagination**: Filter real-time streams by per-process tabs, sort by time, traffic, upload/download, process, destination, protocol, or state, paginate large histories cleanly, and block unwanted domains in one click.
- **Zero-restart hot reload**: Toggle presets, edit rules, add blocked domains, or switch upstream proxies live without dropping the WinDivert handle or active TCP streams.
- **Windows Service + Named Pipe IPC**: Install `BypathService` once as `LocalSystem` so the desktop GUI and CLI talk over `\\.\pipe\bypath-v1` (secured with local-interactive SDDL ACLs and remote-client rejection) without daily Administrator terminals or UAC prompts.
- **Crash-safe & LGPLv3 compliant**: Dynamically loads official signed `WinDivert 2.2+` at runtime via `libloading`; if the process exits or crashes, Windows immediately releases the kernel filter with zero residual network breakage.

## Architecture

- `crates/bypath-core`: Platform-agnostic capture traits, fast process-tree tracker, hot-reloadable two-phase rule engine (L4 process/port/CIDR + L7 SNI/Host domain rules), TLS/HTTP/QUIC sniffer, `smoltcp` userspace TCP/IP netstack, live telemetry hub (`TelemetryHub`), and SOCKS5/HTTP upstream dialers.
- `crates/bypath-windivert`: Windows capture backend using runtime-loaded `WinDivert.dll` + `SOCKET`/`FLOW`/`NETWORK` layers + `GetExtendedTcpTable`/`GetExtendedUdpTable` fallback.
- `crates/bypath-ipc`: Cross-platform NDJSON IPC protocol and secure Windows Named Pipe (`\\.\pipe\bypath-v1`) server/client.
- `crates/bypath-daemon`: Shared `DaemonController`, Windows Service (`BypathService`) SCM integration, and standalone daemon binary (`bypath-daemon.exe`).
- `crates/bypath-cli`: CLI runner (`run`), diagnostics (`doctor`), process inspector (`ps`), service installer (`service`), and Named Pipe remote control (`ctl`).
- `apps/desktop`: Minimalist Tauri v2 + React + TypeScript desktop client (`bypath-gui.exe`) with bilingual `zh-CN`/`en` UI, system tray, process-tabbed connection monitor, domain blocklist manager, one-click presets, running process picker, proxy probe tester, and service manager.

## Quick Start (Windows 10/11 x64)

### 1. Download Signed WinDivert 2.2.2-A Runtime (LGPLv3)

```powershell
.\scripts\fetch-windivert.ps1
```

> **Windows Defender notice**: Windows Defender may heuristically flag WinDivert packages as PUA/HackTool because WinDivert is a kernel packet-interception driver. The fetch script verifies the official SHA-256 hash and Authenticode signature of `WinDivert64.sys`. If quarantined, allow `runtime\windivert\WinDivert.dll` and `WinDivert64.sys` in Windows Security -> Protection history (or add a folder exclusion for `runtime\windivert`).

### 2. Build Core, Daemon, CLI, and Desktop GUI

```powershell
# Build CLI + Background Service Daemon
cargo build --workspace --release

# Build Tauri v2 Desktop GUI (embeds production frontend assets)
npm --prefix apps/desktop install
npm --prefix apps/desktop run tauri:build
```

Outputs:
- `target\release\bypath.exe` (CLI)
- `target\release\bypath-daemon.exe` (Windows Service / IPC daemon)
- `apps\desktop\src-tauri\target\release\bypath-gui.exe` (Desktop GUI)

### 3. Option A — Desktop GUI + Background Windows Service (Recommended)

1. Install and start `BypathService` once (either from **Diagnostics -> Install & Start Service** inside the GUI, or from an Administrator PowerShell):
   ```powershell
   .\target\release\bypath.exe service install
   .\target\release\bypath.exe service start
   ```
2. Launch `apps\desktop\src-tauri\target\release\bypath-gui.exe` as a normal user (no UAC needed).
3. Verify your upstream proxy in **Proxies** (e.g. `127.0.0.1:10808`), toggle presets in **Applications**, add optional blocked domains, and click **Start Proxying**.

### 4. Option B — Standalone CLI

```powershell
# Diagnostics (Administrator PowerShell required for live WinDivert probe)
.\target\release\bypath.exe doctor --proxy socks5://127.0.0.1:10808

# Zero-config per-app proxy with domain blocking
.\target\release\bypath.exe run --proxy socks5://127.0.0.1:10808 --preset antigravity --block-domain "*.doubleclick.net"

# Control running background daemon over Named Pipe IPC
.\target\release\bypath.exe ctl status
.\target\release\bypath.exe ctl start
.\target\release\bypath.exe ctl connections
.\target\release\bypath.exe ctl stop
```

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for WinDivert LGPLv3 dynamic-linking compliance details.
