{
  description = "inode-vpn: H3C SSL VPN client with persistent service (macOS/Linux)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
    };

    # Base source for our own libopenconnect fork (v9.21 + H3C protocol).
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
    openconnect-v921-src,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
        };

        craneLib = crane.mkLib pkgs;

        inode-openconnect = import ./nix/openconnect-h3c-v921.nix {
          inherit pkgs;
          src = openconnect-v921-src;
        };

        inode-module = import ./nix/inode.nix {
          inherit pkgs craneLib;
          src = ./.;
          openconnect = inode-openconnect;
        };

        inode = inode-module.package;
      in {
        packages = {
          default = inode;
          inode = inode;
          inode-openconnect = inode-openconnect;
        };

        checks = {
          inode-clippy = inode-module.clippy;
          inode-fmt = inode-module.fmt;
        };

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
            ];

          shellHook = ''
            echo "🔐 inode-vpn 开发环境"
            echo "  命令:     inode start / stop / restart / status / logs"
            echo "  构建:     cargo build --workspace"
            echo "  引擎:     libopenconnect-h3c（v9.21 fork）"
          '';
        };

        formatter = pkgs.alejandra;
      }
    );
}
