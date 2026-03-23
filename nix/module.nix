{ self, buildtall }:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.nostr-relay;

  # Get packages for the current system
  relayPkg = self.packages.${pkgs.system}.nostr-rs-relay;
  authzPkg = buildtall.packages.${pkgs.system}.relay-authz;

  # Helper to convert Nix attrset to TOML (lib.generators.toTOML doesn't exist)
  toTOML = attrs:
    let
      # Convert a value to TOML format
      valueToTOML = v:
        if builtins.isString v then ''"${v}"''
        else if builtins.isBool v then (if v then "true" else "false")
        else if builtins.isInt v then toString v
        else if builtins.isList v then "[${lib.concatMapStringsSep ", " valueToTOML v}]"
        else toString v;

      # Generate TOML for a section
      sectionToTOML = name: value:
        if builtins.isAttrs value then
          "[${name}]\n" + (lib.concatStringsSep "\n" (lib.mapAttrsToList (k: v:
            if builtins.isAttrs v then ""  # Skip nested attrs (handled separately)
            else "${k} = ${valueToTOML v}"
          ) value))
        else "${name} = ${valueToTOML value}";

      # Collect all sections (top-level attrs that are attrsets)
      sections = lib.filterAttrs (n: v: builtins.isAttrs v) attrs;
      # Collect top-level scalars
      scalars = lib.filterAttrs (n: v: !builtins.isAttrs v) attrs;

      scalarLines = lib.mapAttrsToList (k: v: "${k} = ${valueToTOML v}") scalars;
      sectionLines = lib.mapAttrsToList sectionToTOML sections;
    in
      lib.concatStringsSep "\n\n" (scalarLines ++ sectionLines);

  # Build merged relay config
  mergedConfig = lib.recursiveUpdate {
    # Defaults that can be overridden
    database.data_directory = cfg.dataDir;
  } (lib.recursiveUpdate cfg.settings (
    # If authz is enabled, configure gRPC connection
    lib.optionalAttrs cfg.authz.enable {
      grpc = {
        event_admission_server = "http://${cfg.authz.grpcAddress}";
        restricts_write = true;
      };
    }
  ));

  # Generate relay config.toml
  relayConfig = pkgs.writeText "config.toml" (toTOML mergedConfig);

  # Generate relay-authz config.yaml
  authzConfig = pkgs.writeText "relay-authz.yaml" (lib.generators.toYAML {} {
    log_level = cfg.authz.logLevel;
    database_dir = cfg.dataDir;
    grpc.listen_address = cfg.authz.grpcAddress;
    http.listen_address = cfg.authz.httpAddress;
    http.public_base_url = cfg.authz.publicBaseUrl;
  });

  # Generate seed-admins.yaml
  seedConfig = pkgs.writeText "seed-admins.yaml" (lib.generators.toYAML {} {
    admin_npubs = cfg.authz.adminNpubs;
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
      enable = lib.mkEnableOption "relay-authz authorization sidecar";

      package = lib.mkOption {
        type = lib.types.package;
        default = authzPkg;
        defaultText = lib.literalExpression "buildtall.packages.\${pkgs.system}.relay-authz";
        description = "The relay-authz package to use";
      };

      grpcAddress = lib.mkOption {
        type = lib.types.str;
        default = "[::1]:50051";
        description = "gRPC listen address for authz service";
      };

      httpAddress = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1:8090";
        description = "HTTP listen address for authz admin dashboard";
      };

      publicBaseUrl = lib.mkOption {
        type = lib.types.str;
        default = "https://auth.nostr.io";
        description = "Public base URL for the authz HTTP service";
      };

      logLevel = lib.mkOption {
        type = lib.types.enum [ "DEBUG" "INFO" "WARN" "ERROR" ];
        default = "INFO";
        description = "Log level for the authz service";
      };

      adminNpubs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = "List of admin npubs to seed on startup";
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
    systemd.services.relay-authz = lib.mkIf cfg.authz.enable {
      description = "Relay Authorization Service";
      wantedBy = [ "multi-user.target" ];
      before = [ "nostr-relay.service" ];

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        ExecStart = "${cfg.authz.package}/bin/relay-authz --config ${authzConfig} --seed ${seedConfig}";
        Restart = "always";
        RestartSec = 5;

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ReadWritePaths = [ cfg.dataDir ];
      };
    };

    # Main relay service
    systemd.services.nostr-relay = {
      description = "Nostr Relay (nostr-rs-relay)";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ]
        ++ lib.optional cfg.authz.enable "relay-authz.service";
      requires = lib.optional cfg.authz.enable "relay-authz.service";

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
