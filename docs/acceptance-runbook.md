# inode-vpn 真机验收 Runbook（A31–A40）

> 目标：在 Linux 与 macOS 真机上按命令执行并记录输出，作为发布证据。
> 前置：本机可访问 VPN 网关；远程 Linux 可达且 SSH 可用。

---

## 0. 通用前置

1. 检出发布分支并构建：

   ```sh
   cd ~/inode-cli
   git fetch origin
   git checkout dev/inode-vpn
   nix build .#inode -o /tmp/inode-result
   ```

2. 迁移旧配置（如果还没有 TOML 配置）：

   ```sh
   cd <包含 .auth 的目录>
   /tmp/inode-result/bin/inode config migrate
   ```

3. 记录基准网络：

   ```sh
   # Linux
   ip -4 addr show; ip route show; resolvectl status
   # macOS
   ifconfig; netstat -rn -f inet; scutil --dns
   ```

4. 可选：服务安装 dry-run（不 sudo、不写系统文件）：

   ```sh
   INODE_SERVICE_DRY_RUN=1 /tmp/inode-result/bin/inode enable --now
   INODE_SERVICE_DRY_RUN=1 /tmp/inode-result/bin/inode disable --now
   ```

   确认打印的 unit/plist 路径与 sudo 命令符合预期后再真正安装。

---

## 1. Linux 真机（M3，验收 A31–A37）

### 1.1 安装并启动服务

```sh
sudo /tmp/inode-result/bin/inode enable --now
sleep 5
/tmp/inode-result/bin/inode status --json
```

**通过标准**：状态 `connected`，exit 0。

### 1.2 A31 管理面保活（SSH）

- 从另一台机器（如 Mac）建立 SSH 到本机并保持。
- 依次执行：

  ```sh
  /tmp/inode-result/bin/inode restart
  sleep 3
  sudo systemctl restart inode-vpnd@<uid>
  sleep 5
  # 断网 60s 后插回
  ```

**通过标准**：SSH 会话全程不断开。

### 1.3 A32 路由正确性

连接后执行：

```sh
ip route show
```

**通过标准**：同时存在：
- VPN 网关主机路由（`<VPN_GATEWAY>/32 via <物理网关> dev <物理接口>`）；
- split 路由（来自 NET_EXTEND，如 `192.168.0.0/18 dev tunN`）；
- 物理前缀保护路由（本机 IP/32 + 物理前缀）。

### 1.4 A33 路由撤销

```sh
sudo systemctl stop inode-vpnd@<uid>
ip route show
```

**通过标准**：本会话新增路由全部消失。

### 1.5 A34 持久化 / A35 gc-root

```sh
sudo reboot
# 重启后等待 60s
/tmp/inode-result/bin/inode status --json
nix-collect-garbage
sudo systemctl restart inode-vpnd@<uid>
/tmp/inode-result/bin/inode status --json
```

**通过标准**：两次状态均为 `connected`。

### 1.6 A36 网络变化自愈

```sh
# 拔网 60s 后插回，或切换 Wi-Fi/有线
journalctl -u inode-vpnd@<uid> --since '5 minutes ago' | tail -50
/tmp/inode-result/bin/inode status --json
```

**通过标准**：120s 内恢复 `connected`；journal 出现 `network change detected; reconnecting`。

### 1.7 A37 旧服务迁移

```sh
pgrep -af 'vpn-watch|vpn start|openconnect'
systemctl list-units | grep inode
```

**通过标准**：无旧 `vpn-watch`/双 openconnect；`inode-vpnd@<uid>` 是唯一管理单元。

### 1.8 A29 零泄密

```sh
PASS=$(sed -n 's/^password=//p' .auth)
journalctl -u inode-vpnd@<uid> --no-pager | grep -F "$PASS" && echo LEAK || echo ok
/tmp/inode-result/bin/inode diagnose > /tmp/diag.json
grep -E 'svpnginfo=[A-Za-z0-9@+]' /tmp/diag.json && echo LEAK || echo ok
```

**通过标准**：两处均为 `ok`。

### 1.9 A39 systemd 安全基线

```sh
systemd-analyze security inode-vpnd@<uid>
```

**通过标准**：无 `✗` 高危项；capability 白名单生效。

### 1.10 A30 崩溃恢复

```sh
sudo kill -9 $(systemctl show -p MainPID --value inode-vpnd@<uid>)
sleep 5
/tmp/inode-result/bin/inode status --json
```

**通过标准**：systemd 自动拉起并恢复 `connected`。

### 1.11 24h soak（发布门槛）

```sh
journalctl -u inode-vpnd@<uid> --since '24 hours ago' | grep -E 'Read error|reconnect failed' | wc -l
```

**通过标准**：无未自愈错误；会话仍 `connected`。

---

## 2. macOS 真机（M4，验收 A31–A33、A36、A38）

### 2.1 安装并启动 LaunchDaemon

```sh
sudo /tmp/inode-result/bin/inode enable --now
sleep 5
/tmp/inode-result/bin/inode status --json
```

**通过标准**：`connected`；`launchctl print system/cc.inode.vpn-daemon` 显示 running。

### 2.2 停用旧服务

```sh
launchctl bootout gui/$(id -u)/com.user.vpn-reconnect 2>/dev/null || true
pgrep -af 'vpn-watch|openconnect'
```

**通过标准**：无旧 watchdog；只有 `inode-vpnd` 的 openconnect 会话。

### 2.3 SSH 保活 / 路由正确 / 撤销

```sh
# 保持一条到远程机的 SSH 会话
/tmp/inode-result/bin/inode restart
netstat -rn -f inet | grep -E 'utun|140\.206'
sudo launchctl kickstart -k system/cc.inode.vpn-daemon
/tmp/inode-result/bin/inode stop
netstat -rn -f inet
```

**通过标准**：SSH 不断；会话路由出现后消失。

### 2.4 A38 合盖唤醒 / 换网自愈

```sh
# 合盖 5 分钟后开盖；或切换 Wi-Fi
sleep 30
/tmp/inode-result/bin/inode status --json
tail -50 /var/log/inode-vpn.log
```

**通过标准**：30–120s 内恢复 `connected`。

### 2.5 A29 零泄密

```sh
PASS=$(sed -n 's/^password=//p' .auth)
grep -F "$PASS" /var/log/inode-vpn.log /var/log/inode-vpn.err && echo LEAK || echo ok
/tmp/inode-result/bin/inode diagnose > /tmp/diag.json
grep -E 'svpnginfo=[A-Za-z0-9@+]' /tmp/diag.json && echo LEAK || echo ok
```

---

## 3. 验收记录模板

每项记录：

```text
验收 ID:
平台/机器:
执行时间:
命令与关键输出:
结果: PASS / FAIL
备注:
```

全部通过后，按 `docs/release-checklist.md` 执行发布。
