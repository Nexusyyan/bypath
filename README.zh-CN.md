<p align="center">
  <img src="assets/bypath-banner.svg" alt="Bypath" width="560" />
</p>

<p align="center">
  <strong>基于 Rust 的开源免费应用级透明代理与域名治理工具。</strong><br />
  无需全局 TUN 虚拟网卡、不修改系统路由表；兼顾新手「解压 -> 一次授权 -> 一键开/关」的零门槛体验与 AI Agent / MCP 高自由度 DIY 定制。
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="DESIGN.md">设计规范</a> · <a href="skills/bypath-agent/SKILL.md">Agent 技能指南</a>
</p>

---

Bypath 面向需要“开源版 Proxifier”体验的桌面用户与开发者：无需劫持 `0.0.0.0/0`，即可透明接管指定桌面应用及其**完整派生子进程树**（例如 Electron 客户端 + 原生 Go/Rust/C++ `language_server.exe` 与 MCP 子进程）的 TCP 与 UDP/QUIC 流量，桥接至你现有的上游 SOCKS5 或 HTTP 代理。

## 核心特性

- **GitHub Release 真正开箱即用**：主推完整包 `bypath-<tag>-windows-x64.zip` 由 CI 严格校验并仅内置官方签名版 `WinDivert.dll` + `WinDivert64.sys` 与完整 LGPLv3 许可（剔除易触发误报的 sample 工具），解压双击 `bypath-gui.exe` 后即可在左上角或系统托盘一键切换 **关闭 / 前台模式 / 后台模式**；同步提供 `-nodriver.zip` 纯净无驱动包。
- **零全局路由冲突**：不创建虚拟网卡、不改动系统路由表，与企业 VPN、WSL2、Docker Desktop、Hyper-V 及局域网设备无缝共存。
- **完整进程树追踪与动态防环回**：自动沿父子进程链追踪派生子进程，结合 `NtQueryInformationProcess` 微秒级单 PID 解析、WinDivert `SOCKET` 预热以及上游代理重启自动刷新防环回 PID，稳定接管短生命周期子进程。
- **跨分段后量子 TLS SNI 嗅探与双阶段域名治理**：兼容现代 Chrome/Electron/Go 启用 `X25519MLKEM768` 产生的大包分片 TLS ClientHello，结合 HTTP `Host` 与 QUIC Initial 检测，支持精确域名、通配符（`*.example.com`）与模糊匹配下的拦截（Block）、直连放行（Direct）与指定代理覆写。
- **桌面常驻体验与全链路可观测**：侧栏、诊断页与自定义无边框 CSS 双语托盘弹窗三端联动一键切换 **关闭 / 前台 / 后台** 运行模式（标准用户切换前台自动平滑 UAC 提权交接，切换后台自动按需安装/启动 `BypathService`）；支持可选 **开机自启动**（默认关闭，静默启动到系统托盘）；支持关闭最小化到系统托盘、Win32 单实例防多开、浅色 / 纯炭黑深色 / 跟随系统主题、代理感知应用内一键更新（SHA-256 完整性校验 + 一键「重启以更新」自动无缝恢复 `BypathService` 与运行模式）、全定制圆角控件、全局快捷键、60 秒实时吞吐折线、按进程/域名流量汇总、规则优先级排序与干跑测试、实时日志面板、TOML/CSV 导入导出与绿色便携模式（`portable.json`）。
- **AI Agent 与 CLI 自动化（高自由度 DIY）**：内置原生 Stdio MCP Server（`bypath mcp`）与结构化 JSON 命令行接口（`bypath ctl --json`），可在 Antigravity、Claude Code、Cursor 中直接让 AI 帮您读取系统代理、编排规则、干跑测试、检查版本更新与排查连接日志。

## 快速开始（Windows 10/11 x64）

### 方式一：直接从 GitHub Releases 下载（推荐新手）

1. 前往 [GitHub Releases](https://github.com/Nexusyyan/bypath/releases) 下载并解压 **`bypath-v0.4.3-windows-x64.zip`**。
2. 双击运行 **`bypath-gui.exe`**。
3. 如你的本地代理端口不是默认 `127.0.0.1:10808`（如 Clash 的 `127.0.0.1:7897`），前往「上游代理」页自由编辑或点击「读取系统代理」并保存。
4. 在「应用与域名规则」勾选 **Antigravity** 等预设，在左上角或系统托盘一键选择 **后台**（自动安装/启动 `BypathService`，关闭窗口仍常驻代理且日常免弹窗）或 **前台**（随桌面端启停，标准用户自动弹出一次 UAC 提权）。如需开机自动静默运行，可在「诊断与服务」开启 **开机自启动**（默认关闭）。

> **便携模式**：在 `bypath-gui.exe` 同目录放置 `portable.json`（内容 `{"portable":true}`），所有配置与日志将自动保存于 `./bypath-data/` 目录。

### 方式二：从源码编译

```powershell
# 1. 拉取官方签名版 WinDivert 2.2.2-A 运行时（如需走本地代理可附加 -Proxy http://127.0.0.1:7897）
.\scripts\fetch-windivert.ps1

# 2. 编译核心引擎、后台服务守护进程与 CLI
cargo build --workspace --release

# 3. 编译 Tauri v2 桌面端 GUI（自动内嵌前端资源）
npm --prefix apps/desktop install
npm --prefix apps/desktop run tauri:build
```

### AI Agent / MCP 自动化定制（DIY）

将内置 Stdio MCP Server 接入 Antigravity / Claude Code / Cursor（也可在 GUI「诊断与服务」页一键复制本机路径配置）：

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

或直接调用结构化 JSON 命令行接口：

```powershell
.\bypath.exe ctl --json status
.\bypath.exe ctl --json system-proxy
.\bypath.exe ctl --json rule test --process Antigravity.exe --host aicode.googleapis.com --port 443
.\bypath.exe ctl --json logs --level WARN --limit 50
.\bypath.exe ctl --json update
.\bypath.exe ctl schema
```

完整 Agent 协作流程见 [skills/bypath-agent/SKILL.md](skills/bypath-agent/SKILL.md)。

## 开源协议

采用 [MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) 双许可协议。WinDivert 运行时动态加载与分发合规说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
