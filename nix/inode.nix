# inode: Rust workspace package (CLI + daemon + routectl).
{
  pkgs,
  craneLib,
  src,
  openconnect,
}: let
  lib = pkgs.lib;
in
  craneLib.buildPackage {
    pname = "inode";
    version = "0.1.0";

    inherit src;
    cargoLock = ../Cargo.lock;

    doCheck = true;

    buildInputs = [openconnect];

    env = {
      OPENCONNECT_H3C_LIB = "${openconnect}";
      OPENCONNECT_H3C_INCLUDE = "${openconnect.dev}";
    };

    meta = with lib; {
      description = "H3C SSL VPN client and persistent service (inode-vpn)";
      mainProgram = "inode";
      platforms = platforms.linux ++ platforms.darwin;
    };
  }
