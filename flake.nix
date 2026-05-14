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

        vpn-script = pkgs.writeTextFile {
          name = "vpn";
          executable = true;
          destination = "/bin/vpn";
          text = ''
            #!${pkgs.bash}/bin/bash
            set -euo pipefail

            GATEWAY="''${H3C_GATEWAY:-}"
            AUTH_FILE="$PWD/.auth"
            VPN_DIR="$HOME/.vpn"
            PID_FILE="$VPN_DIR/vpn-pid"
            LOG_FILE="$VPN_DIR/vpn-log"

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

            if [ "''${1:-}" = "stop" ]; then
                if [ -f "$PID_FILE" ]; then
                    sudo kill "$(cat "$PID_FILE")" 2>/dev/null || true
                    rm -f "$PID_FILE"
                fi
                sudo pkill -f "openconnect.*$GATEWAY" 2>/dev/null || true
                echo "VPN 已停止"
                exit 0
            fi

            mkdir -p "$VPN_DIR"

            OPENCONNECT="${pkgs.openconnect_h3c}/bin/openconnect"

            if [ -f "$AUTH_FILE" ]; then
                CERT_ARG=()
                if [ -n "''${SERVERCERT:-}" ] && [ "''${SERVERCERT:-}" != "" ]; then
                    CERT_ARG=(--servercert "$SERVERCERT")
                fi

                printf "%s\n" "$PASSWORD" | \
                    sudo "$OPENCONNECT" --protocol=h3c --passwd-on-stdin --no-dtls "''${CERT_ARG[@]}" "$GATEWAY" -u "$USERNAME" "''${@}" > "$LOG_FILE" 2>&1 &
                PID=$!
                echo "$PID" > "$PID_FILE"

                sleep 3
                if kill -0 "$PID" 2>/dev/null; then
                    echo "VPN 已连接 (pid $PID)"
                else
                    echo "VPN 启动失败，日志："
                    cat "$LOG_FILE"
                    rm -f "$PID_FILE"
                    exit 1
                fi
            else
                echo "未找到 .auth 文件，手动输入凭据"
                sudo "$OPENCONNECT" --protocol=h3c --no-dtls "$GATEWAY" "''${@}"
            fi
          '';
        };
      in
      {
        packages = {
          default = pkgs.openconnect_h3c;
          openconnect-h3c = pkgs.openconnect_h3c;
          vpn = vpn-script;
        };

        devShells.default = pkgs.mkShell {
          name = "openconnect-h3c";
          packages = with pkgs; [
            openconnect_h3c
            vpn-script
          ];


          shellHook = ''
            echo "🔐 VPN 环境就绪 (openconnect-h3c)"
            echo "  启动: vpn"
            echo "  停止: vpn stop"
            echo "  或直接: nix run .#vpn"
          '';
        };

        formatter = pkgs.nixfmt;
      }
    );
}
