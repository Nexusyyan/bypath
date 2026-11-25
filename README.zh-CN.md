<p align="center">
  <img src="assets/bypath-banner.svg" alt="Bypath" width="560" />
</p>

<p align="center">
  <strong>基于 Rust 的开源免费应用级透明代理与域名治理工具。</strong><br />
  无需开启全局 TUN 虚拟网卡，也不修改系统路由表，即可精准接管指定应用及其完整子进程树流量。
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="DESIGN.md">设计规范</a>
</p>

---

Bypath 面向需要“开源版 Proxifier”体验的桌面用户与开发者：无需劫持 `0.0.0.0/0`，即可透明接管指定桌面应用及其**完整派生子进程树**（例如 Electron 客户端 + 原生 Go/Rust/C++ `language_server.exe` 与 MCP 子进程）的 TCP 与 UDP/QUIC 流量，桥接至你现有的上游 SOCKS5 或 HTTP 代理，并提供域名级拦截与实时连接监控。

## 核心特性

- **零全局路由冲突**：不创建虚拟网卡、不改动系统路由表，与企业 VPN、WSL2、Docker Desktop、Hyper-V 及局域网设备无缝共存。
- **完整进程树与目录匹配**：自动沿父子进程链追踪派生子进程，结合 Windows `NtQueryInformationProcess` 微秒级单 PID 解析与 WinDivert `SOCKET` 非阻塞预热，防止短生命周期子进程漏接管。
- **SNI 协议嗅探与域名拦截**：内置 TLS SNI、HTTP `Host` 与 QUIC Initial 嗅探，既可还原真实域名交由上游代理远端解析（规避本地 DNS 污染），也可在拨号前实时按精确域名、通配符（`*.example.com`）或模糊规则拦截指定域名。
- **QUIC / UDP 治理**：支持 RFC 1928 SOCKS5 `UDP ASSOCIATE`；当上游代理仅支持 TCP 或命中域名拦截规则时自动优雅丢弃 QUIC（UDP/443），触发客户端秒级回落至可嗅探的 TCP/TLS 连接。
- **连接监控进程分页、多维排序与一键拦截**：连接监控页提供顶部进程 Tab 分页与底部页码分页，支持按时间、总流量、上行、下行、进程、目标、协议或状态排序，并可在连接详情中一键拦截对应域名或通配符根域名。
- **零中断热重载**：开关应用预设、增删域名拦截、修改规则或切换上游代理立即生效，不释放 WinDivert 句柄、不断开现有 TCP 活跃连接。
- **Windows 后台服务 + Named Pipe IPC**：一次性安装 `BypathService`（`LocalSystem` 权限）后，桌面端与 CLI 即可通过带本地交互式 SDDL 权限控制的 `\\.\pipe\bypath-v1` 安全通信，日常使用免管理员终端、免 UAC 弹窗。
- **崩溃安全与合规架构**：通过 `libloading` 动态加载官方微软签名版 `WinDivert 2.2+`（符合 LGPLv3 与 MIT/Apache-2.0 合规要求），进程退出或崩溃时内核毫秒级回收过滤句柄，无任何断网残留。

## 快速开始（Windows 10/11 x64）

### 1. 拉取官方签名版 WinDivert 2.2.2-A 运行时

```powershell
.\scripts\fetch-windivert.ps1
```

> **Windows Defender 误报说明**：由于 `WinDivert` 属于内核级网络包拦截驱动，Windows Defender 可能会将其启发式标记为 PUA/HackTool。脚本内置官方 SHA-256 哈希校验与 `WinDivert64.sys` 数字签名核验。如遇隔离，请在「Windows 安全中心 -> 病毒和威胁防护 -> 保护历史记录」中允许，或将 `runtime\windivert` 加入排除目录。

### 2. 编译 CLI、后台服务与桌面 GUI

```powershell
# 编译核心引擎、后台服务守护进程与 CLI
cargo build --workspace --release

# 编译 Tauri v2 桌面端 GUI（自动内嵌前端资源）
npm --prefix apps/desktop install
npm --prefix apps/desktop run tauri:build
```

产物位置：
- `target\release\bypath.exe`（CLI 与遥控工具）
- `target\release\bypath-daemon.exe`（Windows Service / Named Pipe 守护进程）
- `apps\desktop\src-tauri\target\release\bypath-gui.exe`（桌面端 GUI）

### 3. 启动桌面端与后台服务（推荐）

1. 在桌面端「诊断与服务」页面点击 **一键安装并启动服务**（或在管理员 PowerShell 执行 `.\target\release\bypath.exe service install` 与 `.\target\release\bypath.exe service start`）；
2. 普通权限直接启动 `bypath-gui.exe`，在「上游代理」核对代理地址（默认 `127.0.0.1:10808`），在「应用与域名规则」勾选预设或配置拦截域名，点击 **开启代理** 即可。

### 4. 独立 CLI 用法

```powershell
# 环境与代理体检
.\target\release\bypath.exe doctor --proxy socks5://127.0.0.1:10808

# 零配置接管指定应用预设并拦截特定域名
.\target\release\bypath.exe run --proxy socks5://127.0.0.1:10808 --preset antigravity --block-domain "*.doubleclick.net"

# 通过 Named Pipe 遥控后台服务
.\target\release\bypath.exe ctl status
.\target\release\bypath.exe ctl connections
```

## 开源协议

采用 [MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) 双许可协议。WinDivert 运行时动态加载合规说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
