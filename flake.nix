{
  description = "Nostr Relay written in Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane.url = "github:ipetkov/crane";
  };

  outputs = inputs@{ self, ... }:
    (inputs.flake-utils.lib.eachDefaultSystem (system:
      let
        # Import nixpkgs with rust-overlay
        overlays = [ (import inputs.rust-overlay) ];
        pkgs = import inputs.nixpkgs {
          inherit system overlays;
        };

        # Use Rust stable latest (1.81+ required by home@0.5.11)
        rustToolchain = pkgs.rust-bin.stable.latest.default;

        # Override crane's toolchain via overrideToolchain (current API)
        craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (p: rustToolchain);
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
      in
      {
        checks = {
          inherit crate;
        };
        packages = {
          default = crate;
          nostr-rs-relay = crate;
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
      nixosModules.default = import ./nix/module.nix {
        inherit self;
      };
      nixosModules.nostr-relay = self.nixosModules.default;
    };
}
