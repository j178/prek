{
  description = "Better pre-commit, re-engineered in Rust";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }: let
    version = "0.4.5";

    # ponytail: only the 4 standard Nix systems are exposed; upstream also ships
    # musl/arm/armv7/riscv64/s390x tarballs but those map to no stdenv system.
    # Upgrade path: add entries here + bump version + refresh SRI hashes.
    assets = {
      "x86_64-linux" = {
        file = "prek-x86_64-unknown-linux-gnu.tar.gz";
        sha256 = "sha256-3IbhjlMlFt1inuYhq3a8TEdhBSIZsn468eR8v5pxonA=";
      };
      "aarch64-linux" = {
        file = "prek-aarch64-unknown-linux-gnu.tar.gz";
        sha256 = "sha256-akFDf9aGQd556qw9bdZmf6OFEsk6c/ZhombeKxJVexs=";
      };
      "x86_64-darwin" = {
        file = "prek-x86_64-apple-darwin.tar.gz";
        sha256 = "sha256-sK/Iu+pp1hu/5JEAOU35nu0VqcRnWwbJk2QX5lqoixY=";
      };
      "aarch64-darwin" = {
        file = "prek-aarch64-apple-darwin.tar.gz";
        sha256 = "sha256-7Ukgdi8OPbBxY8Q3r2IqHUkF4a1wU2kqSVVRuc6SVJ8=";
      };
    };

    systems = builtins.attrNames assets;
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);

    prekFor = system: let
      pkgs = nixpkgs.legacyPackages.${system};
      asset = assets.${system};
    in pkgs.stdenv.mkDerivation {
      pname = "prek";
      inherit version;

      src = pkgs.fetchurl {
        url = "https://github.com/j178/prek/releases/download/v${version}/${asset.file}";
        sha256 = asset.sha256;
      };

      sourceRoot = ".";

      nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.autoPatchelfHook ];
      buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.stdenv.cc.cc.lib ];

      dontConfigure = true;
      dontBuild = true;

      installPhase = ''
        runHook preInstall
        mkdir -p "$out/bin"
        cp prek-*/prek "$out/bin/prek"
        chmod +x "$out/bin/prek"
        runHook postInstall
      '';

      meta = with pkgs.lib; {
        description = "Better pre-commit, re-engineered in Rust";
        homepage = "https://github.com/j178/prek";
        downloadPage = "https://github.com/j178/prek/releases";
        license = licenses.mit;
        mainProgram = "prek";
        platforms = systems;
        sourceProvenance = [ sourceTypes.binaryNativeCode ];
      };
    };
  in {
    packages = forAllSystems (system: rec {
      prek = prekFor system;
      default = prek;
    });

    apps = forAllSystems (system: {
      prek = {
        type = "app";
        program = "${prekFor system}/bin/prek";
      };
      default = {
        type = "app";
        program = "${prekFor system}/bin/prek";
      };
    });
  };
}
