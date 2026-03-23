{
  description = "Nostr Relay written in Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, ... }:
    (inputs.flake-utils.lib.eachDefaultSystem (system:
      let
        # Import nixpkgs with rust-overlay
        overlays = [ (import inputs.rust-overlay) ];
        pkgs = import inputs.nixpkgs {
          inherit system overlays;
        };

        # Use Rust 1.81 or later (required by home@0.5.11)
        # Using stable.latest should give us at least 1.81
        rustToolchain = pkgs.rust-bin.stable.latest.minimal;

        # Override pkgs to use the newer Rust toolchain
        pkgsWithRust = pkgs.extend (final: prev: {
          rustc = rustToolchain;
          cargo = rustToolchain;
        });

        lib = pkgs.lib;
        craneLib = inputs.crane.mkLib pkgsWithRust;
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
          nativeBuildInputs = [ 
            pkgs.pkg-config 
            pkgs.protobuf 
          ];
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
        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
            pkgs.pkg-config
            pkgs.protobuf
          ];
        };
      })) // {
      # System-independent outputs
      nixosModules.default = import ./nix/module.nix { inherit self; };
      nixosModules.nostr-relay = self.nixosModules.default;
    };
}
