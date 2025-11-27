{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.nostr-relay;

  # Get packages for the current system
  relayPkg = self.packages.${pkgs.system}.nostr-rs-relay;
  authzPkg = self.packages.${pkgs.system}.nip42-authz;

  # Generate relay config.toml
  relayConfig = pkgs.writeText "config.toml" (lib.generators.toTOML {} (
    lib.recursiveUpdate {
      # Defaults that can be overridden
      database.data_directory = cfg.dataDir;
    } (lib.recursiveUpdate cfg.settings (
      # If authz is enabled, configure gRPC connection
      lib.optionalAttrs cfg.authz.enable {
        grpc = {
          event_admission_server = "http://${cfg.authz.listenAddress}";
          restricts_write = true;
        };
      }
    ))
  ));

  # Generate authz policy-config.toml
  authzConfig = pkgs.writeText "policy-config.toml" (lib.generators.toTOML {} {
    log_level = cfg.authz.logLevel;
    listen_address = cfg.authz.listenAddress;
    allowed_npubs = cfg.authz.allowedNpubs;
  });

in
{
  options.services.nostr-relay = {
    enable = lib.mkEnableOption "nostr-rs-relay Nostr relay server";

    package = lib.mkOption {
      type = lib.types.package;
      default = relayPkg;
      defaultText = lib.literalExpression "self.packages.\${pkgs.system}.nostr-rs-relay";
      description = "The nostr-rs-relay package to use";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "nostr";
      description = "User under which the relay runs";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "nostr";
      description = "Group under which the relay runs";
    };

    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/nostr-relay";
      description = "Directory for relay data (SQLite database)";
    };

    settings = lib.mkOption {
      type = lib.types.attrsOf lib.types.anything;
      default = {};
      description = ''
        Relay configuration settings (converted to TOML).
        See config.toml for available options.
      '';
      example = lib.literalExpression ''
        {
          info = {
            relay_url = "wss://relay.example.com/";
            name = "My Relay";
            description = "A private Nostr relay";
          };
          network = {
            address = "127.0.0.1";
            port = 7777;
          };
          authorization = {
            nip42_auth = true;
          };
        }
      '';
    };

    authz = {
      enable = lib.mkEnableOption "NIP-42 authorization sidecar";

      package = lib.mkOption {
        type = lib.types.package;
        default = authzPkg;
        defaultText = lib.literalExpression "self.packages.\${pkgs.system}.nip42-authz";
        description = "The nip42-authz package to use";
      };

      listenAddress = lib.mkOption {
        type = lib.types.str;
        default = "[::1]:50051";
        description = "gRPC listen address for authz service";
      };

      logLevel = lib.mkOption {
        type = lib.types.enum [ "DEBUG" "INFO" "WARN" "ERROR" ];
        default = "ERROR";
        description = "Log level for the authz service";
      };

      allowedNpubs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = "List of npubs allowed to publish";
        example = [ "npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx" ];
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # Create user and group
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = cfg.dataDir;
      createHome = true;
      description = "Nostr relay service user";
    };

    users.groups.${cfg.group} = {};

    # Authz sidecar service (if enabled)
    systemd.services.nip42-authz = lib.mkIf cfg.authz.enable {
      description = "NIP-42 Authorization Service";
      wantedBy = [ "multi-user.target" ];
      before = [ "nostr-relay.service" ];

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        ExecStart = "${cfg.authz.package}/bin/nip42-authz";
        Restart = "always";
        RestartSec = 5;

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ReadWritePaths = [ cfg.dataDir ];
      };

      # Copy config to working directory before starting
      preStart = ''
        cp ${authzConfig} ${cfg.dataDir}/policy-config.toml
        chmod 600 ${cfg.dataDir}/policy-config.toml
      '';
    };

    # Main relay service
    systemd.services.nostr-relay = {
      description = "Nostr Relay (nostr-rs-relay)";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ]
        ++ lib.optional cfg.authz.enable "nip42-authz.service";
      requires = lib.optional cfg.authz.enable "nip42-authz.service";

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        ExecStart = "${cfg.package}/bin/nostr-rs-relay --config ${relayConfig}";
        Restart = "always";
        RestartSec = 5;

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.dataDir ];
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
  };
}
