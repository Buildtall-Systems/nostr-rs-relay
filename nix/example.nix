# Example NixOS configuration using the relay module
#
# To use this in your NixOS configuration:
#
# 1. Add the flake input to your flake.nix:
#    inputs.nostr-rs-relay.url = "github:your-org/nostr-rs-relay";
#
# 2. Import the module in your NixOS configuration:
#    imports = [ inputs.nostr-rs-relay.nixosModules.default ];
#
# 3. Configure the service as shown below.

{ config, pkgs, ... }:

{
  services.nostr-relay = {
    enable = true;

    user = "nostr";
    group = "nostr";
    dataDir = "/var/lib/nostr-relay";

    settings = {
      info = {
        relay_url = "wss://relay.example.com/";
        name = "My Relay";
        description = "A private Nostr relay";
        pubkey = "dd81a8bacbab0b5c3007d1672fb8301383b4e9583d431835985057223eb298a5";
        contact = "admin@example.com";
      };

      network = {
        address = "127.0.0.1"; # Behind reverse proxy
        port = 7777;
      };

      authorization = {
        nip42_auth = true;
      };

      # Optional: Rate limiting
      limits = {
        messages_per_sec = 5;
        max_event_bytes = 131072;
        max_ws_message_bytes = 131072;
      };
    };

    # Enable the NIP-42 authorization sidecar
    authz = {
      enable = true;
      logLevel = "INFO";
      listenAddress = "[::1]:50051";
      allowedNpubs = [
        "npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx"
        # Add more npubs as needed
      ];
    };
  };

  # Optional: nginx reverse proxy with TLS
  # services.nginx = {
  #   enable = true;
  #   virtualHosts."relay.example.com" = {
  #     forceSSL = true;
  #     enableACME = true;
  #     locations."/" = {
  #       proxyPass = "http://127.0.0.1:7777";
  #       proxyWebsockets = true;
  #     };
  #   };
  # };
}
