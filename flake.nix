{
  description = "Nostr Relay written in Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, ... }:
    (inputs.flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = inputs.nixpkgs.legacyPackages.${system};
        lib = pkgs.lib;
        craneLib = inputs.crane.mkLib pkgs;
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            (pkgs.lib.hasSuffix "\.proto" path) ||
            # Default filter from crane (allow .rs files)
            (craneLib.filterCargoSources path type)
          ;
        };
        crate = craneLib.buildPackage {
          name = "nostr-rs-relay";
          inherit src;
          nativeBuildInputs = [ pkgs.pkg-config pkgs.protobuf ];
        };

        nip42-authz = pkgs.buildGoModule {
          pname = "nip42-authz";
          version = "0.1.0";
          src = ./go-nip42-authz;

          # Use vendored dependencies (no hash needed)
          vendorHash = null;

          # Proto files are pre-generated, no preBuild needed

          ldflags = [ "-s" "-w" ];

          # Rename the binary from rs-relay-auth-server to nip42-authz
          postInstall = ''
            mv $out/bin/rs-relay-auth-server $out/bin/nip42-authz
          '';

          meta = {
            description = "NIP-42 Authorization gRPC service for nostr-rs-relay";
            license = lib.licenses.mit;
          };
        };
      in
      {
        checks = {
          inherit crate;
        };
        packages = {
          default = crate;
          nostr-rs-relay = crate;
          inherit nip42-authz;
        };
        formatter = pkgs.nixpkgs-fmt;
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
        };
      })) // {
      # System-independent outputs
      nixosModules.default = import ./nix/module.nix { inherit self; };
      nixosModules.nostr-relay = self.nixosModules.default;
    };
}
