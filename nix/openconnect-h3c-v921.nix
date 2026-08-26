# inode-openconnect: our H3C fork, based on upstream openconnect v9.21.
#
# The fork is represented as upstream release tarball + patch/h3c-v921.patch.
{
  pkgs,
  src,
  patch ? ../patch/h3c-v921.patch,
}:
pkgs.openconnect.overrideAttrs (old: {
  pname = "openconnect-h3c";
  version = "9.21-h3c";

  inherit src;

  patches = (old.patches or []) ++ [patch];

  postPatch =
    (old.postPatch or "")
    + ''
      # vpnc-script is shipped separately from upstream releases; reuse the
      # one from nixpkgs' openconnect source like the legacy overlay does.
      cp ${pkgs.openconnect.src}/vpnc-script vpnc-script 2>/dev/null || true
    '';

  meta =
    (old.meta or {})
    // {
      description = "OpenConnect with H3C SSL VPN protocol support (inode-vpn fork, v9.21)";
    };
})
