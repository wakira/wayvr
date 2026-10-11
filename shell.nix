{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  # Rust toolchain + pkg-config + libclang/bindgen for ovr_overlay (autocxx)
  nativeBuildInputs = with pkgs; [
    cargo
    rustc
    pkg-config
    rustPlatform.bindgenHook
  ];

  buildInputs = with pkgs; [
    alsa-lib
    dav1d
    dbus
    fontconfig
    libGL
    libX11
    libxcb
    libXext
    libinput
    libXrandr
    libxkbcommon
    openxr-loader
    pipewire
    shaderc
    udev
    vulkan-loader
    wayland
    openvr
  ];

  # wgui loads libshaderc at runtime for shader compilation
  env.SHADERC_LIB_DIR = "${pkgs.lib.getLib pkgs.shaderc}/lib";

  shellHook = ''
    # Some NixOS setups export CARGO_HOME=/cargo, which is not user-writable.
    if [ -n "''${CARGO_HOME:-}" ] && [ ! -w "$CARGO_HOME" ]; then
      echo "shell.nix: CARGO_HOME=$CARGO_HOME is not writable, using $HOME/.cargo"
      unset CARGO_HOME
    fi

    # SteamVR dlopen()s its vrclient.so, which has no RUNPATH and needs the
    # graphics/X runtime libs. nix-shell does not expose buildInputs on the
    # loader path, so add them here (otherwise: VRInitError_Init_VRClientDLLNotFound).
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (with pkgs; [
      libGL
      libdrm
      util-linux
      libX11
      libxcb
      libXext
      libXrandr
      libxkbcommon
      udev
      vulkan-loader
    ])}:''${LD_LIBRARY_PATH:-}"
  '';
}
