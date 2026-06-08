# macOS sleepwatcher 守护进程：唤醒时触发 `vpn reconnect`
{pkgs}:
pkgs.writeShellScriptBin "vpn-watch" ''
  exec ${pkgs.sleepwatcher}/bin/sleepwatcher -w "$HOME/.vpn/vpn reconnect"
''
