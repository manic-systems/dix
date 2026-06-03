{
  description = "Dix - Diff Nix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      eachSystem = lib.genAttrs [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      pkgsFor = eachSystem (
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

            doCheck = false;
            strictDeps = true;
          };
        }
      ) pkgsFor;

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

            cargo-flamegraph

            (inputs.fenix.packages.${system}.combine (
              with inputs.fenix.packages.${system};
              [
                stable.cargo
                stable.clippy
                stable.rust-analyzer
                stable.rustc

                # nightly rustfmt for better formatting
                default.rustfmt
              ]
            ))
          ];

          env.RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      }) pkgsFor;
    };
}
