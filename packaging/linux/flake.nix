{
  description = "Ctrlmate — Modern PC telemetry, lighting, cooling & Home Assistant daemon";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          ctrlmated = pkgs.rustPlatform.buildRustPackage {
            pname = "ctrlmated";
            version = "0.4.0";
            src = ./../..;
            cargoLock.lockFile = ./../../Cargo.lock;
            cargoBuildFlags = [ "-p" "ctrlmate-daemon" ];
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.udev pkgs.hidapi ];
            meta = with pkgs.lib; {
              description = "Ctrlmate cross-platform PC daemon for Home Assistant";
              license = licenses.agpl3Plus;
              platforms = platforms.linux;
            };
          };
          default = self.packages.${system}.ctrlmated;
        });
    };
}
