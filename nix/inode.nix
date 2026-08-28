# inode: Rust workspace package (CLI + daemon + routectl).
#
# Returns an attrset:
#   package - buildPackage derivation
#   clippy  - cargoClippy check (-D warnings)
#   fmt     - cargoFmt check
{
  pkgs,
  craneLib,
  src,
  openconnect,
}: let
  lib = pkgs.lib;

  common = {
    pname = "inode";
    version = "1.0.0";
    inherit src;
    cargoLock = ../Cargo.lock;
    buildInputs = [openconnect];
    env = {
      OPENCONNECT_H3C_LIB = "${openconnect}";
      OPENCONNECT_H3C_INCLUDE = "${openconnect.dev}";
    };
  };

  cargoArtifacts = craneLib.buildDepsOnly common;

  package = craneLib.buildPackage (common
    // {
      inherit cargoArtifacts;
      doCheck = true;

      postInstall = ''
        ln -s ${openconnect}/bin/openconnect "$out/bin/openconnect-h3c"
      '';

      meta = with lib; {
        description = "H3C SSL VPN client and persistent service (inode-vpn)";
        mainProgram = "inode";
        platforms = platforms.linux ++ platforms.darwin;
      };
    });

  clippy = craneLib.cargoClippy (common
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    });

  fmt = craneLib.cargoFmt common;
in {
  inherit package clippy fmt;
}
