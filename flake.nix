{
  description = "OpenConnect with H3C SSL VPN protocol support";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    openconnect-h3c-src = {
      url = "gitlab:vimacs.hacks/openconnect/h3cssl";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, openconnect-h3c-src, ... }:
    let
      overlay = final: prev: {
        openconnect_h3c = prev.openconnect.overrideAttrs (old: {
          pname = "openconnect-h3c";
          version = "9.12-h3c";
          src = openconnect-h3c-src;

          patches = (old.patches or []) ++ [ ./patch/h3c-busy-loop.patch ];

          postPatch = (old.postPatch or "") + ''
            cp ${prev.openconnect.src}/vpnc-script vpnc-script 2>/dev/null || true
          '';

          meta = (old.meta or { }) // {
            description = "OpenConnect with H3C SSL VPN protocol support (MR !397)";
          };
        });
      };
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
        };
        lib = pkgs.lib;
        isDarwin = pkgs.stdenv.isDarwin;

        # sleepwatcher 守护进程：唤醒时触发 `vpn reconnect`。
        # -w 后的命令交给 sleepwatcher 用 sh -c 执行，$HOME 在此处由本脚本展开。
        # 仅 macOS：sleepwatcher 依赖 macOS 电源事件。
        vpn-watch =
          if isDarwin then
            pkgs.writeShellScriptBin "vpn-watch" ''
              exec ${pkgs.sleepwatcher}/bin/sleepwatcher -w "$HOME/.vpn/vpn reconnect"
            ''
          else null;

        # 仅在 macOS 启动时刷新指向 vpn-watch 的稳定 symlink（if 惰性，linux 不求值）。
        watchLink =
          if isDarwin
          then ''ln -sf ${vpn-watch}/bin/vpn-watch "$VPN_DIR/vpn-watch"''
          else "";

        vpn-script = pkgs.writeTextFile {
          name = "vpn";
          executable = true;
          destination = "/bin/vpn";
          text = ''
            #!${pkgs.bash}/bin/bash
            set -euo pipefail

            # LaunchAgent 环境 PATH 极简，补上系统目录以便找到 sudo/pkill/launchctl
            export PATH="$PATH:/usr/bin:/bin:/usr/sbin:/sbin"

            GATEWAY="''${H3C_GATEWAY:-}"
            VPN_DIR="$HOME/.vpn"
            PID_FILE="$VPN_DIR/vpn-pid"
            LOG_FILE="$VPN_DIR/vpn-log"
            DIR_FILE="$VPN_DIR/vpn-dir"

            mkdir -p "$VPN_DIR"
            # 维护家目录下的稳定入口，LaunchAgent / hook 始终指向这里，
            # flake 重建后只要再跑一次 vpn 就会刷新指向最新版本。
            ln -sf "$0" "$VPN_DIR/vpn"
            ${watchLink}

            # 仿 systemd 用法：首个参数为子命令，其余参数透传
            CMD="''${1:-}"
            if [ "$#" -gt 0 ]; then shift; fi

            usage() {
                echo "用法: vpn <命令> [参数]" >&2
                echo "" >&2
                echo "  start             连接 VPN（在含 .auth 的目录中运行）" >&2
                echo "  stop              断开 VPN" >&2
                echo "  restart           断开后在当前目录重新连接" >&2
                echo "  status            查看连接与自动重连服务状态" >&2
                echo "  enable [--now]    安装开盖唤醒自动重连服务（macOS）；--now 同时立即连接" >&2
                echo "  disable [--now]   移除自动重连服务；--now 同时立即断开" >&2
                echo "  install-sudoers   写入 openconnect 的 sudo 免密规则（macOS，一次性）" >&2
                echo "  uninstall-sudoers 移除 sudo 免密规则" >&2
            }

            # reconnect：回到上次成功启动的目录后重连（hook 唤醒时调用，内部命令）
            if [ "$CMD" = "reconnect" ]; then
                if [ ! -s "$DIR_FILE" ]; then
                    echo "未找到上次 VPN 工作目录，请先在项目目录手动运行一次 vpn start" >&2
                    exit 1
                fi
                TARGET=$(cat "$DIR_FILE")
                if [ ! -d "$TARGET" ]; then
                    echo "记录的 VPN 目录已不存在: $TARGET" >&2
                    exit 1
                fi
                cd "$TARGET"
                "$0" stop || true
                exec "$0" start
            fi

            # enable：安装开盖唤醒自动重连服务（macOS）；--now 同时立即连接
            if [ "$CMD" = "enable" ]; then
                if [ "$(uname)" != "Darwin" ]; then
                    echo "enable 仅支持 macOS" >&2
                    exit 1
                fi
                PLIST="$HOME/Library/LaunchAgents/com.user.vpn-reconnect.plist"
                mkdir -p "$HOME/Library/LaunchAgents"
                {
                    echo '<?xml version="1.0" encoding="UTF-8"?>'
                    echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
                    echo '<plist version="1.0"><dict>'
                    echo '<key>Label</key><string>com.user.vpn-reconnect</string>'
                    echo '<key>ProgramArguments</key>'
                    echo "<array><string>$VPN_DIR/vpn-watch</string></array>"
                    echo '<key>RunAtLoad</key><true/>'
                    echo '<key>KeepAlive</key><true/>'
                    echo '</dict></plist>'
                } > "$PLIST"
                launchctl unload "$PLIST" 2>/dev/null || true
                launchctl load "$PLIST"
                echo "已安装开盖唤醒自动重连服务"
                echo "提示：需先配置 openconnect 的 sudo 免密（vpn install-sudoers），否则唤醒重连会因等待 sudo 密码而失败"
                if [ "''${1:-}" = "--now" ]; then
                    exec "$0" start
                fi
                exit 0
            fi

            # disable：移除自动重连服务；--now 同时立即断开
            if [ "$CMD" = "disable" ]; then
                PLIST="$HOME/Library/LaunchAgents/com.user.vpn-reconnect.plist"
                launchctl unload "$PLIST" 2>/dev/null || true
                rm -f "$PLIST"
                echo "已移除开盖唤醒自动重连服务"
                if [ "''${1:-}" = "--now" ]; then
                    exec "$0" stop
                fi
                exit 0
            fi

            # status：查看连接、工作目录与自动重连服务状态（仿 systemctl status，
            # 退出码：已连接 0 / 未连接 3）
            if [ "$CMD" = "status" ]; then
                RC=3
                if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
                    echo "● vpn - 已连接 (active, running)"
                    echo "  Main PID: $(cat "$PID_FILE")"
                    RC=0
                else
                    echo "○ vpn - 未连接 (inactive)"
                fi
                if [ -s "$DIR_FILE" ]; then
                    echo "  工作目录: $(cat "$DIR_FILE")"
                fi
                if [ "$(uname)" = "Darwin" ]; then
                    PLIST="$HOME/Library/LaunchAgents/com.user.vpn-reconnect.plist"
                    if [ -f "$PLIST" ]; then
                        if launchctl list 2>/dev/null | grep -q com.user.vpn-reconnect; then
                            echo "  开盖自动重连: enabled (已加载)"
                        else
                            echo "  开盖自动重连: enabled (未加载)"
                        fi
                    else
                        echo "  开盖自动重连: disabled"
                    fi
                fi
                if [ -s "$LOG_FILE" ]; then
                    echo "  最近日志:"
                    tail -n 3 "$LOG_FILE" | sed 's/^/    /'
                fi
                exit "$RC"
            fi

            # install-sudoers：写入 openconnect / kill / pkill 的 sudo 免密规则，
            # 使 hook 唤醒重连无需交互输入 sudo 密码（首次写入需一次 sudo 授权）
            if [ "$CMD" = "install-sudoers" ]; then
                if [ "$(uname)" != "Darwin" ]; then
                    echo "install-sudoers 目前仅适配 macOS 命令路径" >&2
                    exit 1
                fi
                USER_NAME=$(id -un)
                TMP=$(mktemp)
                {
                    echo "# openconnect-h3c VPN 免密规则，由 vpn install-sudoers 生成"
                    echo "$USER_NAME ALL=(root) NOPASSWD: /nix/store/*/bin/openconnect, /bin/kill, /usr/bin/pkill"
                } > "$TMP"
                if ! visudo -c -f "$TMP" >/dev/null; then
                    echo "sudoers 语法校验失败，未写入" >&2
                    rm -f "$TMP"
                    exit 1
                fi
                echo "即将写入 /etc/sudoers.d/vpn（需要一次 sudo 授权）："
                sed 's/^/  /' "$TMP"
                sudo install -m 0440 -o root -g wheel "$TMP" /etc/sudoers.d/vpn
                rm -f "$TMP"
                echo "已安装 sudo 免密规则"
                exit 0
            fi

            if [ "$CMD" = "uninstall-sudoers" ]; then
                sudo rm -f /etc/sudoers.d/vpn
                echo "已移除 sudo 免密规则"
                exit 0
            fi

            AUTH_FILE="$PWD/.auth"
            if [ -f "$AUTH_FILE" ]; then
                auth_val() { sed -n "s/^$1=//p" "$AUTH_FILE" | head -1; }
                USERNAME=$(auth_val username)
                PASSWORD=$(auth_val password)
                SERVERCERT=$(auth_val servercert)
                AUTH_GATEWAY=$(auth_val gateway)
                if [ -n "''${AUTH_GATEWAY:-}" ]; then
                    GATEWAY="$AUTH_GATEWAY"
                fi
            fi

            if [ "$CMD" = "stop" ]; then
                if [ -f "$PID_FILE" ]; then
                    sudo kill "$(cat "$PID_FILE")" 2>/dev/null || true
                    rm -f "$PID_FILE"
                fi
                sudo pkill -f "openconnect.*$GATEWAY" 2>/dev/null || true
                echo "VPN 已停止"
                exit 0
            fi

            # restart：当前目录下断开后重连
            if [ "$CMD" = "restart" ]; then
                "$0" stop || true
                exec "$0" start "$@"
            fi

            # 到此仅接受 start（或裸命令显示用法）
            if [ -z "$CMD" ]; then
                usage
                exit 0
            fi
            if [ "$CMD" != "start" ]; then
                echo "未知命令: $CMD" >&2
                usage
                exit 1
            fi

            OPENCONNECT="${pkgs.openconnect_h3c}/bin/openconnect"

            if [ -f "$AUTH_FILE" ]; then
                CERT_ARG=()
                if [ -n "''${SERVERCERT:-}" ] && [ "''${SERVERCERT:-}" != "" ]; then
                    CERT_ARG=(--servercert "$SERVERCERT")
                fi

                printf "%s\n" "$PASSWORD" | \
                    sudo "$OPENCONNECT" --protocol=h3c --passwd-on-stdin --no-dtls "''${CERT_ARG[@]}" "$GATEWAY" -u "$USERNAME" "$@" > "$LOG_FILE" 2>&1 &
                PID=$!
                echo "$PID" > "$PID_FILE"

                sleep 3
                if kill -0 "$PID" 2>/dev/null; then
                    # 记录本次工作目录，供 reconnect 在任意目录下回到此处
                    echo "$PWD" > "$DIR_FILE"
                    echo "VPN 已连接 (pid $PID)"
                else
                    echo "VPN 启动失败，日志："
                    cat "$LOG_FILE"
                    rm -f "$PID_FILE"
                    exit 1
                fi
            else
                echo "未找到 .auth 文件，手动输入凭据"
                sudo "$OPENCONNECT" --protocol=h3c --no-dtls "$GATEWAY" "$@"
            fi
          '';
        };
      in
      {
        packages = {
          default = pkgs.openconnect_h3c;
          openconnect-h3c = pkgs.openconnect_h3c;
          vpn = vpn-script;
        } // lib.optionalAttrs isDarwin { inherit vpn-watch; };

        devShells.default = pkgs.mkShell {
          name = "openconnect-h3c";
          packages = with pkgs; [
            openconnect_h3c
            vpn-script
          ] ++ lib.optional isDarwin vpn-watch;


          shellHook = ''
            echo "🔐 VPN 环境就绪 (openconnect-h3c)，仿 systemd 用法"
            echo "  启动:   vpn start"
            echo "  停止:   vpn stop"
            echo "  重启:   vpn restart"
            echo "  状态:   vpn status"
            echo "  sudo 免密:    vpn install-sudoers   (macOS，一次性授权)"
            echo "  开盖自动重连: vpn enable            (macOS，依赖上面的免密)"
            echo "  装好并立即连: vpn enable --now"
            echo "  关闭:   vpn disable [--now] / vpn uninstall-sudoers"
            echo "  或直接: nix run .#vpn -- start"
          '';
        };

        formatter = pkgs.nixfmt;
      }
    );
}
