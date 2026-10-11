{
  pkgs ? import <nixpkgs> { },
  # Which cargo features to build. Defaults match the SteamVR + desktop use case.
  # Add "openxr" to also support Monado/WiVRn (pulls in libmonado/openxr-loader).
  features ? [
    "wayland"
    "x11"
    "openvr"
    "openxr"
    "osc"
  ],
}:

let
  # Runtime libs that SteamVR dlopen()s (vrclient.so has no RUNPATH), plus the
  # Vulkan loader. Without these you get VRInitError_Init_VRClientDLLNotFound.
  steamRuntimeLibs = with pkgs; [
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
  ];
in
pkgs.rustPlatform.buildRustPackage {
  pname = "wayvr";
  version = "26.8.0";

  src = pkgs.lib.cleanSourceWith {
    src = ./.;
    filter =
      path: _type:
      let
        base = baseNameOf (toString path);
      in
      !(builtins.elem base [
        "target"
        ".git"
      ]);
  };

  cargoHash = "sha256-1452r/EUSwCgdLhU0xmNCYUKJD2girlaUMRx21P/5SU=";

  # The binary lives in the `wayvr` workspace member; build only that package.
  buildAndTestSubdir = "wayvr";

  buildNoDefaultFeatures = true;
  buildFeatures = features;

  nativeBuildInputs = with pkgs; [
    pkg-config
    rustPlatform.bindgenHook
    makeWrapper
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
    wayland
    openvr
  ];

  env.SHADERC_LIB_DIR = "${pkgs.lib.getLib pkgs.shaderc}/lib";

  doCheck = false;

  postFixup = ''
    wrapProgram $out/bin/wayvr \
      --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath steamRuntimeLibs}"
  '';

  meta = {
    description = "Access your Wayland/X11 desktop from SteamVR/Monado/WiVRn (OpenVR + OpenXR)";
    homepage = "https://github.com/wayvr-org/wayvr";
    license = pkgs.lib.licenses.gpl3Only;
    platforms = pkgs.lib.platforms.linux;
    mainProgram = "wayvr";
  };
}
