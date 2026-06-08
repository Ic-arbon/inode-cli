# vpn 命令：读取纯 bash 脚本模板，注入 Nix store 路径后打包
#
# 参数：
#   pkgs      — nixpkgs 实例（需包含 openconnect_h3c overlay）
#   watchLink — 写入 ~/.vpn/vpn-watch symlink 的 shell 命令（Linux 下为空）
{
  pkgs,
  watchLink ? "",
}: let
  template = builtins.readFile ./vpn.sh.in;
  script =
    builtins.replaceStrings
    ["@bash@" "@openconnect@" "@watchLink@"]
    ["${pkgs.bash}" "${pkgs.openconnect_h3c}" watchLink]
    template;
in
  pkgs.writeTextFile {
    name = "vpn";
    executable = true;
    destination = "/bin/vpn";
    text = script;
  }
