{
  description = "inode-vpn: H3C SSL VPN client with persistent service (macOS/Linux)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
    };

    # Legacy openconnect fork (upstream v9.01 + MR!397). Kept for the old
    # `vpn` shell command until the new stack takes over.
    openconnect-h3c-src = {
      url = "gitlab:vimacs.hacks/openconnect/h3cssl";
      flake = false;
    };

    # Base source for our own fork.
    openconnect-v921-src = {
      url = "https://www.infradead.org/openconnect/download/openconnect-9.21.tar.gz";
      flake = false;
    };
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    crane,
    openconnect-h3c-src,
    openconnect-v921-src,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        legacyOverlay = import ./nix/overlay.nix {src = openconnect-h3c-src;};

        pkgs = import nixpkgs {
          inherit system;
          overlays = [legacyOverlay];
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

        craneLib = crane.mkLib pkgs;

        inode = import ./nix/inode.nix {
          inherit pkgs craneLib;
          src = ./.;
          openconnect = inode-openconnect;
        };

        inode-openconnect = import ./nix/openconnect-h3c-v921.nix {
          inherit pkgs;
          src = openconnect-v921-src;
        };

        # Transition alias: exposes `vpn` -> `inode` without replacing the
        # legacy `vpn` shell package yet (full switch happens in M3/M4).
        vpn-inode = pkgs.runCommand "vpn-inode" {} ''
          mkdir -p "$out/bin"
          ln -s ${inode}/bin/inode "$out/bin/vpn"
        '';
      in {
        packages =
          {
            default = inode;
            inode = inode;
            inode-openconnect = inode-openconnect;
            # Legacy packages; keep working until M3/M4 migration is done.
            openconnect-h3c = pkgs.openconnect_h3c;
            vpn = vpn-script;
            vpn-inode = vpn-inode;
          }
          // lib.optionalAttrs isDarwin {inherit vpn-watch;};

        devShells.default = pkgs.mkShell {
          name = "inode-vpn";
          packages = with pkgs;
            [
              cargo
              rustc
              rustfmt
              clippy
              inode
              inode-openconnect
              vpn-script
            ]
            ++ lib.optional isDarwin vpn-watch;

          shellHook = ''
            echo "🔐 inode-vpn 开发环境"
            echo "  新工具:   inode start / stop / restart / status / logs"
            echo "  旧命令:   vpn start ...（过渡兼容，M3/M4 后由 inode 接管）"
            echo "  构建:     cargo build --workspace"
            echo "  fork:     openconnect --protocol=h3c ..."
          '';
        };

        formatter = pkgs.alejandra;
      }
    );
}
