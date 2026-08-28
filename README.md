# inode-vpn

H3C SSL VPN 客户端 + 持久化服务（macOS / Linux）。基于自己的
`libopenconnect-h3c` fork（upstream v9.21 + H3C 协议），由 Rust 实现。

## 当前状态

| 里程碑 | 状态 |
|---|---|
| M0 基线（workspace / fork 构建 / CI） | ✅ GitHub Actions 四平台 CI 通过 |
| M1 fork 协议引擎（组帧 / 心跳 / DPD / 重连） | ✅ 已实测通过 |
| M2 daemon + CLI 核心 | ✅ 已实测通过（script-tun 模式） |
| M3 Linux 服务化（routectl / systemd / netwatch） | ✅ 真机验收通过（A31–A37/A29/A39/A30） |
| M4 macOS 服务化（routectl / LaunchDaemon） | ✅ 真机验收通过（A31/A33/A36/A38/A29） |
| M5 安全收敛 / 发布 | 🟡 仅剩 24h soak 与发布动作 |

## 架构速览

```text
inode (CLI)
  │ UDS · JSON-RPC 2.0 · peer-uid 校验
  ▼
inode-vpnd (root daemon, systemd / LaunchDaemon 管理)
  ├─ libopenconnect-h3c (fork: h3c 协议 / 隧道 / 心跳 / 重连)
  └─ inode-routectl (vpnc-script 替代：tun / 路由 / DNS)
```

完整设计见 `docs/architecture.md`。

## 构建

需要启用 flakes 的 Nix：

```sh
nix build .#inode          # inode / inode-vpnd / inode-routectl
nix build .#inode-openconnect   # 自己的 fork
nix develop                 # 进入开发环境（含 cargo / rustc）
```

Rust 单独构建（需要 fork 的 lib 路径）：

```sh
nix build .#inode-openconnect -o oc
OPENCONNECT_H3C_LIB="$(readlink oc)" \
OPENCONNECT_H3C_INCLUDE="$(nix eval --raw .#inode-openconnect.dev)" \
cargo build --workspace
```

## 配置

`~/.config/inode-vpn/config.toml`（必须 0600）：

```toml
[gateway]
url = "vpn.example.com:2000"        # 与旧 .auth gateway 同语义
servercert = "pin-sha256:..."       # SPKI pin，可留空后 discover-cert

[credentials]
username = "..."
password = "..."

[network]
keepalive = "auto"                  # 或秒数；0 关闭心跳/DPD
preserve_cidrs = []                 # 额外保护网段，如 ["192.168.0.0/23"]
dns = "server"                      # server | ignore

[service]
autostart = true
restart_delay = 10
```

从旧 `.auth` 迁移：

```sh
inode config migrate
```

## 使用

```sh
inode start
inode stop
inode restart
inode status [--json]      # exit: 0=Connected, 3=Stopped, 1=Failed
inode discover-cert [--force]  # TOFU 获取/更新 pin-sha256 (SPKI)
inode logs [-f]
inode enable [--now]       # Linux: systemd unit；macOS: LaunchDaemon
inode disable [--now]
inode diagnose             # 脱敏诊断包
```

服务安装要求 root（首次会通过 `sudo` 交互授权），可执行文件通过稳定链接
`/var/lib/inode-vpn/current` 引用，避免 Nix store GC 导致 unit 失效。

## H3C 协议要点（实测）

- 控制面：`GET /svpn/index.cgi` → `GET /client_getinfo.cgi` →
  `POST /_xml/login.cgi`（XML form-encoded）→ `NET_EXTEND /`。
- 数据面：TLS 流内 `type:u16-LE + len:u16-BE + payload`，`type=1` 为 IPv4。
- 心跳：`02 00 00 00`，服务器应答 `02 02 00 00`；间隔来自
  `KEEPALIVETIME` 头（默认 30s）。
- 判活不使用 ping；`checkonline.cgi` 作为独立控制面探针。

## 安全

- 密码与 `svpnginfo` cookie 永不进入 argv、日志、IPC 响应或 diagnose 包。
- 配置 0600 强制；管理 socket 校验 peer uid。
- systemd 单元启用 capability 白名单、`ProtectSystem=strict`、
  `ProtectHome=read-only`；macOS 使用 root LaunchDaemon。

## 已知待办

- Linux/macOS 真机验收矩阵（见开发计划 A31–A40）。
- macOS DNS 服务名发现待补。
