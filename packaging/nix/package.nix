{
  lib,
  stdenv,
  rustPlatform,
  callPackage,
  runCommand,
  zig_0_15,
  zstd,
  pkg-config,
  git,
  cctools ? null,
  xcbuild ? null,
}:

let
  manifest = lib.importTOML ../../Cargo.toml;
  zigDeps = callPackage ../../vendor/libghostty-vt/build.zig.zon.nix {
    name = "bus-libghostty-vt-zig-cache";
    inherit zstd;
    linkFarm =
      name: entries:
      runCommand name { } ''
        mkdir -p $out
        ${lib.concatMapStringsSep "\n" (entry: ''
          cp -rL ${entry.path} $out/${entry.name}
        '') entries}
      '';
  };
  darwinToolchain = lib.optionals stdenv.hostPlatform.isDarwin [
    cctools
    xcbuild
  ];
in
rustPlatform.buildRustPackage {
  pname = "bus";
  version = manifest.package.version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.intersection (lib.fileset.fromSource (lib.sources.cleanSource ../..)) (
      lib.fileset.unions [
        ../../assets
        # The orchestrator embeds its prompt, docs and workflows at compile time.
        ../../orchestration
        ../../workflows
        ../../src
        # Unit tests embed fixtures outside src; keep those compile-time inputs too.
        ../../tests/fixtures
        ../../vendor/libghostty-vt
        ../../vendor/libghostty-vt.vendor.json
        ../../vendor/portable-pty
        ../../build.rs
        ../../Cargo.lock
        ../../Cargo.toml
      ]
    );
  };

  cargoLock = {
    lockFile = ../../Cargo.lock;
  };

  nativeBuildInputs = [
    git
    pkg-config
  ] ++ darwinToolchain;

  env = {
    LIBGHOSTTY_VT_OPTIMIZE = "ReleaseFast";
    LIBGHOSTTY_VT_SIMD = "true";
    LIBGHOSTTY_VT_ZIG_SYSTEM_DIR = zigDeps;
    ZIG = lib.getExe zig_0_15;
  };

  preBuild = ''
    export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-global-cache"
    export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"
  '';

  # The local gate covers Rust tests. This package is build-only so it
  # validates packaging inputs without duplicating the test suite.
  doCheck = false;

  meta = {
    description = "Coordinate selected AI coding agents in native terminal rooms (built on Herdr)";
    license = lib.licenses.asl20;
    mainProgram = "bus";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
