{
  description = "Ctrlmate: PC telemetry, lighting, cooling and Home Assistant control";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      # Single source of truth: [workspace.package].version.
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      src = self;
      mkPkgs = system: nixpkgs.legacyPackages.${system};

      mkCtrlmate = pkgs:
        let
          common = {
            inherit version src;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.udev pkgs.hidapi ];
            doCheck = true;
          };
          meta = with pkgs.lib; {
            license = licenses.agpl3Plus;
            homepage = "https://github.com/Thomas-TP/Ctrlmate";
            platforms = platforms.linux;
          };
        in
        {
          ctrlmated = pkgs.rustPlatform.buildRustPackage (common // {
            pname = "ctrlmated";
            cargoBuildFlags = [ "-p" "ctrlmate-daemon" ];
            cargoTestFlags = [ "-p" "ctrlmate-daemon" ];
            meta = meta // { description = "Ctrlmate hardware daemon"; mainProgram = "ctrlmated"; };
          });
          ctrlmate-agent = pkgs.rustPlatform.buildRustPackage (common // {
            pname = "ctrlmate-agent";
            cargoBuildFlags = [ "-p" "ctrlmate-agent" ];
            cargoTestFlags = [ "-p" "ctrlmate-agent" ];
            meta = meta // { description = "Ctrlmate session agent"; mainProgram = "ctrlmate-agent"; };
          });
        };
    in
    {
      packages = forAllSystems (system:
        let p = mkCtrlmate (mkPkgs system); in
        {
          inherit (p) ctrlmated ctrlmate-agent;
          default = p.ctrlmated;
        });

      # The Tauri desktop app is intentionally not a Nix package yet: it needs the front-end
      # dependency hash and webkitgtk wrapping. Use the devShell to build it:
      #   nix develop -c bash -c 'cd apps/desktop && bun install --frozen-lockfile && cd ../.. && cargo build --release -p ctrlmate-desktop'

      nixosModules.default = { config, lib, pkgs, ... }:
        let cfg = config.services.ctrlmate; in
        {
          options.services.ctrlmate = {
            enable = lib.mkEnableOption "the Ctrlmate hardware daemon";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.ctrlmated;
              description = "The ctrlmated package.";
            };
            agentPackage = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.ctrlmate-agent;
              description = "The ctrlmate-agent package.";
            };
            enableAgent = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Run ctrlmate-agent as a systemd user service in graphical sessions.";
            };
            openFirewall = lib.mkEnableOption "opening TCP 9487 (Home Assistant TLS) and UDP 5353 (mDNS) in the firewall";
          };

          config = lib.mkIf cfg.enable {
            users.groups.ctrlmate = { };
            users.users.ctrlmate = {
              isSystemUser = true;
              group = "ctrlmate";
              home = "/var/lib/ctrlmate";
              description = "Ctrlmate hardware daemon";
            };
            boot.kernelModules = [ "i2c-dev" ];

            # Exact VID:PID rules, mirrored from packaging/linux/99-ctrlmate.rules.
            services.udev.extraRules = builtins.readFile ./packaging/linux/99-ctrlmate.rules;

            systemd.services.ctrlmated = {
              description = "Ctrlmate hardware daemon";
              wantedBy = [ "multi-user.target" ];
              after = [ "network-online.target" ];
              wants = [ "network-online.target" ];
              serviceConfig = {
                ExecStart = "${cfg.package}/bin/ctrlmated --data-dir /var/lib/ctrlmate";
                User = "ctrlmate";
                Group = "ctrlmate";
                Restart = "on-failure";
                RestartSec = 3;
                TimeoutStopSec = 15;
                StateDirectory = "ctrlmate";
                StateDirectoryMode = "0750";
                ConfigurationDirectory = "ctrlmate";
                RuntimeDirectory = "ctrlmate";
                CapabilityBoundingSet = "";
                NoNewPrivileges = true;
                ProtectSystem = "strict";
                ProtectHome = true;
                PrivateTmp = true;
                ProtectKernelTunables = true;
                ProtectKernelModules = true;
                ProtectControlGroups = true;
                RestrictNamespaces = true;
                LockPersonality = true;
                RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" "AF_NETLINK" ];
                SystemCallArchitectures = "native";
                UMask = "0027";
                DevicePolicy = "closed";
                DeviceAllow = [
                  "char-hidraw rw"
                  "char-i2c-dev rw"
                  "char-nvidia-frontend rw"
                  "char-nvidia-uvm rw"
                  "char-nvidiactl rw"
                ];
              };
            };

            systemd.user.services.ctrlmate-agent = lib.mkIf cfg.enableAgent {
              description = "Ctrlmate session agent";
              wantedBy = [ "graphical-session.target" ];
              partOf = [ "graphical-session.target" ];
              after = [ "graphical-session.target" ];
              serviceConfig = {
                ExecStart = "${cfg.agentPackage}/bin/ctrlmate-agent";
                Restart = "on-failure";
                RestartSec = 3;
              };
            };

            networking.firewall = lib.mkIf cfg.openFirewall {
              allowedTCPPorts = [ 9487 ];
              allowedUDPPorts = [ 5353 ];
            };
          };
        };

      devShells = forAllSystems (system:
        let pkgs = mkPkgs system; in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo rustc rustfmt clippy pkg-config
              udev hidapi
              bun nodejs
              gtk3 webkitgtk_4_1 libsoup_3 libayatana-appindicator
              cargo-deny cargo-audit
              python3
            ];
          };
        });

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) ctrlmated ctrlmate-agent;
      });
    };
}
