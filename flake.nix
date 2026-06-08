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

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    openconnect-h3c-src,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        overlay = import ./nix/overlay.nix {src = openconnect-h3c-src;};

        pkgs = import nixpkgs {
          inherit system;
          overlays = [overlay];
        };

        lib = pkgs.lib;
        isDarwin = pkgs.stdenv.isDarwin;

        vpn-watch =
          if isDarwin
          then import ./nix/vpn-watch.nix {inherit pkgs;}
          else null;

        watchLink =
          if isDarwin
          then ''ln -sf ${vpn-watch}/bin/vpn-watch "$VPN_DIR/vpn-watch"''
          else "";

        vpn-script = import ./nix/vpn-script.nix {
          inherit pkgs watchLink;
        };
      in {
        packages =
          {
            default = pkgs.openconnect_h3c;
            openconnect-h3c = pkgs.openconnect_h3c;
            vpn = vpn-script;
          }
          // lib.optionalAttrs isDarwin {inherit vpn-watch;};

        devShells.default = pkgs.mkShell {
          name = "openconnect-h3c";
          packages = with pkgs;
            [
              openconnect_h3c
              vpn-script
            ]
            ++ lib.optional isDarwin vpn-watch;

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

        formatter = pkgs.alejandra;
      }
    );
}
