{
  src,
  patch ? ../patch/h3c-busy-loop.patch,
}: final: prev: {
  openconnect_h3c = prev.openconnect.overrideAttrs (old: {
    pname = "openconnect-h3c";
    version = "9.12-h3c";
    inherit src;

    patches = (old.patches or []) ++ [patch];

    postPatch =
      (old.postPatch or "")
      + ''
        cp ${prev.openconnect.src}/vpnc-script vpnc-script 2>/dev/null || true
      '';

    meta =
      (old.meta or {})
      // {
        description = "OpenConnect with H3C SSL VPN protocol support (MR !397)";
      };
  });
}
