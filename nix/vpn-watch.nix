# macOS launchd 守护进程：唤醒即时重连 + 常驻巡检重连
#
# 由 com.user.vpn-reconnect.plist 以 KeepAlive 拉起，脱离终端/登录会话，
# 是唤醒后能稳定重连的关键（终端 fork 的子进程扛不过合盖/关终端）。
#
#   sleepwatcher -w  → 开盖/唤醒瞬间触发一次带退避的 reconnect（低延迟快路径）
#   vpn supervise    → 常驻轮询，掉线即重连（兜底安全网，也覆盖睡眠外的掉线）
#
# 两者都经 ~/.vpn/vpn symlink 调用，每次 exec 重解析，重建 flake 后不踩 GC 路径。
{pkgs}:
pkgs.writeShellScriptBin "vpn-watch" ''
  VPN="$HOME/.vpn/vpn"

  ${pkgs.sleepwatcher}/bin/sleepwatcher -w "$VPN reconnect" &
  SW=$!

  "$VPN" supervise &
  SUP=$!

  # launchd 卸载/重启时（SIGTERM）连带收回两个子进程，避免 sleepwatcher 泄漏叠加
  trap 'kill "$SW" "$SUP" 2>/dev/null || true' EXIT INT TERM

  # 任一子进程退出即结束本进程，交给 launchd KeepAlive 干净重启
  wait -n "$SW" "$SUP" 2>/dev/null || wait "$SUP" || true
''
