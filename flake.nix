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
      in
      {
        packages = {
          default = pkgs.openconnect_h3c;
          openconnect-h3c = pkgs.openconnect_h3c;
        };

        devShells.default = pkgs.mkShell {
          name = "openconnect-h3c";
          packages = with pkgs; [
            openconnect_h3c
          ];

          H3C_GATEWAY = "vpn.example.com:443";

          shellHook = ''
            export PATH="$PWD/bin:$PATH"

            echo "🔐 VPN 环境就绪 (openconnect-h3c)"
            echo ""
            echo "  启动: vpn"
            echo "  停止: vpn-stop"
            echo ""
          '';
        };

        formatter = pkgs.nixfmt;
      }
    );
}
