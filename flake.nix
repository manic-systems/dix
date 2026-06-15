{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";


#foobar
  outputs =
    {
      self,
      nixpkgs,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      forEachSystem = lib.genAttrs [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      pkgsForEach = forEachSystem (
        system:
        import nixpkgs {
          localSystem.system = system;
        }
      );
    in
    {
      packages = lib.mapAttrs (
        system: pkgs:
        let
          fs = lib.fileset;

          src = fs.difference (fs.gitTracked ./.) (
            fs.unions [
              ./.envrc
              ./.rustfmt.toml
              ./flake.lock
              (fs.fileFilter (file: lib.strings.hasInfix ".git" file.name) ./.)
              (fs.fileFilter (file: file.hasExt "md") ./.)
              (fs.fileFilter (file: file.hasExt "nix") ./.)
            ]
          );
        in
        {
          default = self.packages.${system}.dix;

          dix = pkgs.rustPlatform.buildRustPackage {
            name = "dix";

            src = fs.toSource {
              root = ./.;
              fileset = src;
            };

            cargoLock = {
              lockFile = ./Cargo.lock;
              allowBuiltinFetchGit = true;
            };

            buildType = "release";

            strictDeps = true;
          };
        }
      ) pkgsForEach;

      devShells = lib.mapAttrs (system: pkgs: {
        default = self.devShells.${system}.dix;

        dix = pkgs.mkShell {
          packages = with pkgs; [
            # A nice compiler daemon.
            bacon
            # Better tests.
            cargo-nextest
            # TOML formatting.
            taplo

            cargo
            clippy
            rust-analyzer
            rustc
            (rustfmt.override { asNightly = true; })
          ];

          env.RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      }) pkgsForEach;
    };
}
