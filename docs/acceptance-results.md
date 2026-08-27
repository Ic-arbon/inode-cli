# inode-vpn 真机验收结果

> 记录每项验收的实测证据。密码与 cookie 不落盘；所有日志均经脱敏检查。

## 测试矩阵

| 编号 | 项目 | 平台 | 结果 | 证据 |
|---|---|---|---|---|
| A31 | 管理面保活 | Linux (Fedora 44) | ✅ | SSH 会话中 `inode restart` / `systemctl restart` 后状态 `connected`，会话未断 |
| A32 | 路由正确性 | Linux | ✅ | `ip route show` 同时存在 `140.206.103.26 via 192.168.1.1 dev wlo1`、`192.168.0.128 via …`、`192.168.0.0/23 via …`、split `192.168.0.0/18 dev tun0` |
| A33 | 路由撤销 | Linux | ✅ | `sudo systemctl stop inode-vpnd@1000.service` 后 `ip route show` 仅剩系统原有路由；stop 不再超时 |
| A34 | 重启持久化 | Linux | ✅ | `sudo reboot` 后服务 enabled+active，60s 内自动 `connected` |
| A35 | gc-root | Linux | ✅ | `nix-collect-garbage` 删除 2532 路径后 `/nix/var/nix/gcroots/inode-vpn-1000` 目标仍在；`systemctl restart` 后 `connected` |
| A36 | 网络变化自愈 | Linux | ✅ | `ip link set wlo1 down; sleep 60; up` 后 120s 内恢复 `connected`；journal 出现 `network change detected; reconnecting` 并 `openconnect session reconnected` |
| A37 | 旧服务迁移 | Linux | ✅ | 无 vpn-watch / 旧 openconnect 进程；`inode-vpnd@1000.service` 为唯一管理单元 |
| A29 | 零泄密 | Linux | ✅ | journal 全量 grep 密码 = ok；`inode diagnose` grep `svpnginfo=` = ok |
| A39 | systemd 安全基线 | Linux | ✅ | `systemd-analyze security` 4.5 `OK`；capability 白名单生效（DAC/CHOWN/SETFCAP 等 ✓） |
| A30 | 崩溃恢复 | Linux | ✅ | `kill -9 MainPID` 后 systemd 5s 内拉起，12s 内自动重连 `connected`（新 IP） |
| 24h soak | 发布门槛 | Linux | ⏳ | 运行中；当前无未自愈错误 |
| A31 | 管理面保活 | macOS (arm64) | ✅ | `sudo launchctl kickstart -k system/cc.inode.vpn-daemon` 后自动恢复 `connected` |
| A33 | 路由撤销 | macOS | ✅ | `inode stop` 后 `netstat -rn` 中 inode 的 utun 路由全部消失 |
| A29 | 零泄密 | macOS | ✅ | `/var/log/inode-vpn.log`/`.err` grep 密码 = ok；`inode diagnose` grep `svpnginfo=` = ok |
| A36 | 网络变化自愈 | macOS | ✅ | 关 Wi-Fi 60s 再打开后 120s 内恢复 `connected`；日志 `network change detected; reconnecting` + `openconnect session reconnected` |
| A38 | 合盖唤醒自愈 | macOS | ⏳ | 合盖 5 分钟开盖后需在 120s 内恢复 `connected` |
| T0.4/A6 | 四平台 CI | — | ⏳ | 以 `.github/workflows/ci.yml` 在 GitHub Actions 上跑四平台（用户确认以 GitHub CI 为准） |

## 关键修复（本验收轮）

- service installer：`SUDO_UID`/`INODE_SERVICE_UID` 识别真实用户；unit 实例 `inode-vpnd@<uid>`。
- stable link 指向包根目录（消除 `current/bin/bin` 双重路径）。
- systemd `RuntimeDirectoryMode=0755`，IPC socket 显式 0666 + SO_PEERCRED 鉴权。
- daemon 用 `getpwuid(uid)` 解析目标用户配置；服务以目标用户 + ambient `CAP_NET_ADMIN` 运行，无需 DAC bypass。
- `inode restart` 等待引擎线程退出后再启动；SIGTERM 不再 join 阻塞的 IPC listener。
- `inode-routectl` 从 `reason` 环境变量取 phase（openconnect 经 `sh -c` exec），支持 `attempt-reconnect`。
- 物理前缀归一化为网络地址（`192.168.0.128/23` → `192.168.0.0/23`）再执行 `ip route replace`。
- netwatch 只监听物理接口 address/link，忽略 tun/lo，避免自家路由触发重连风暴。
- macOS netwatch 基于 SystemConfiguration dynamic store（Global/IPv4、Service/IPv4、接口 Link），过滤 utun/lo0/awdl 等虚拟接口。
- 网络恢复时：运行中引擎发 `OC_CMD_PAUSE` 立即重连；已 `failed` 的引擎用已存配置自动重启（覆盖断网超过 reconnect 窗口的场景）。
- macOS `inode-routectl` `pre-init` 删除旧 VPN 网关主机路由，避免换网后 `connect()` 报 `EADDRNOTAVAIL`。
- `enable` 注册真实 Nix gc-root `/nix/var/nix/gcroots/inode-vpn-<uid>`。

## 执行环境

- Linux 真机：OB714，Fedora 44，systemd 259，x86_64，用户 tyd(uid 1000)。
- macOS 真机：Apple Silicon，LaunchDaemon `cc.inode.vpn-daemon`，用户 tyd(uid 501)。
- 网关：H3C `140.206.103.26:2000`；会话隧道 `10.1.1.0/24`，keepalive 30s。
- Linux 受测包：`/nix/store/46yymn3vqk9pa9qxxykznnpf6qmfsd8j-inode-0.1.0`（commit 71221df）。
- macOS 受测包：`/nix/store/z3pr79482i4xici7z7bvqnmc026r5z6f-inode-0.1.0`（commit 4ecadf9）。
