{
  description = "Weather";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "weather";
            version = "0.0.1";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
          };
          nativeBuildInputs = with pkgs; [
            tailwindcss_4
          ];
          preBuild = ''
            echo "Compiling Tailwind CSS..."
            tailwindcss -i ./src/styles.css -o ./static/styles.css --minify
          '';
        }
      );

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [ rustc cargo rustfmt clippy tailwindcss_4 ];
          };
        }
      );

      nixosModules.default = { pkgs, ... }@args:
        let
          weatherPkg = self.packages.${pkgs.system}.default;
        in
        import ./module.nix { inherit weatherPkg; } args;
    };
}
