{ self }:
{ config, lib, pkgs, ... }:

let
  relayPkg = self.packages.${pkgs.system}.nostr-rs-relay;
  # Helper to convert Nix attrset to TOML (lib.generators.toTOML doesn't exist)
  toTOML = attrs:
    let
      valueToTOML = v:
        if builtins.isString v then ''"${v}"''
        else if builtins.isBool v then (if v then "true" else "false")
        else if builtins.isInt v then toString v
        else if builtins.isList v then "[${lib.concatMapStringsSep ", " valueToTOML v}]"
        else toString v;

      sectionToTOML = name: value:
        if builtins.isAttrs value then
          "[${name}]\n" + (lib.concatStringsSep "\n" (lib.mapAttrsToList (k: v:
            if builtins.isAttrs v then ""
            else "${k} = ${valueToTOML v}"
          ) value))
        else "${name} = ${valueToTOML value}";

      sections = lib.filterAttrs (n: v: builtins.isAttrs v) attrs;
      scalars = lib.filterAttrs (n: v: !builtins.isAttrs v) attrs;

      scalarLines = lib.mapAttrsToList (k: v: "${k} = ${valueToTOML v}") scalars;
      sectionLines = lib.mapAttrsToList sectionToTOML sections;
    in
      lib.concatStringsSep "\n\n" (scalarLines ++ sectionLines);

  instanceOpts = { name, config, ... }: {
    options = {
      enable = lib.mkEnableOption "this nostr relay instance";

      package = lib.mkOption {
        type = lib.types.package;
        default = relayPkg;
        defaultText = lib.literalExpression "self.packages.\${pkgs.system}.nostr-rs-relay";
        description = "The nostr-rs-relay package to use";
      };

      user = lib.mkOption {
        type = lib.types.str;
        default = name;
        description = "User under which the relay runs (defaults to instance name)";
      };

      group = lib.mkOption {
        type = lib.types.str;
        default = name;
        description = "Group under which the relay runs (defaults to instance name)";
      };

      createUser = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to create the system user and group";
      };

      dataDir = lib.mkOption {
        type = lib.types.path;
        default = "/var/lib/${name}";
        description = "Directory for relay data";
      };

      configFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Pre-built config.toml path. When null, generated from settings.";
      };

      settings = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = {};
        description = ''
          Relay configuration settings (converted to TOML).
          Ignored when configFile is set.
        '';
      };

      socketActivation = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Enable systemd socket activation (default: on)";
        };

        listenAddress = lib.mkOption {
          type = lib.types.str;
          default = "127.0.0.1";
          description = "Address for the systemd socket to listen on";
        };

        port = lib.mkOption {
          type = lib.types.port;
          default = 8080;
          description = "Port for the systemd socket to listen on";
        };
      };

    };
  };

  enabledInstances = lib.filterAttrs (_: inst: inst.enable) config.services.nostr-relay.instances;

  mkInstanceConfig = name: inst:
    let
      mergedConfig = lib.recursiveUpdate {
        database.data_directory = inst.dataDir;
      } inst.settings;

      generatedRelayConfig = pkgs.writeText "${name}-relay.toml" (toTOML mergedConfig);
      relayConfig = if inst.configFile != null then inst.configFile else generatedRelayConfig;

    in
    {
      users = lib.mkIf inst.createUser {
        users.${inst.user} = {
          isSystemUser = true;
          group = inst.group;
          home = inst.dataDir;
          createHome = true;
          description = "${name} nostr relay service user";
        };
        groups.${inst.group} = {};
      };

      socket = lib.optionalAttrs inst.socketActivation.enable {
        "${name}-relay" = {
          description = "${name} Nostr Relay Socket";
          wantedBy = [ "sockets.target" ];
          socketConfig = {
            ListenStream = "${inst.socketActivation.listenAddress}:${toString inst.socketActivation.port}";
            FreeBind = true;
            ReusePort = true;
            NoDelay = true;
            FileDescriptorName = "http";
          };
        };
      };

      relayService = {
        "${name}-relay" = {
          description = "${name} Nostr Relay (nostr-rs-relay)";
          wantedBy = lib.optional (!inst.socketActivation.enable) "multi-user.target";
          after = [ "network.target" ];
          requires = lib.optional inst.socketActivation.enable "${name}-relay.socket";

          serviceConfig = {
            Type = "notify";
            NotifyAccess = "main";
            User = inst.user;
            Group = inst.group;
            WorkingDirectory = inst.dataDir;
            ExecStart = "${inst.package}/bin/nostr-rs-relay --config ${relayConfig}";
            Restart = "always";
            RestartSec = 5;
            NoNewPrivileges = true;
            ProtectSystem = "strict";
            ReadWritePaths = [ inst.dataDir ];
            ProtectHome = true;
            PrivateTmp = true;
          };
        };
      };
    };

  instanceConfigs = lib.mapAttrs mkInstanceConfig enabledInstances;

in
{
  options.services.nostr-relay.instances = lib.mkOption {
    type = lib.types.attrsOf (lib.types.submodule instanceOpts);
    default = {};
    description = "Named nostr-rs-relay instances";
  };

  config = lib.mkIf (enabledInstances != {}) {
    users = lib.mkMerge (lib.mapAttrsToList (_: ic: ic.users) instanceConfigs);

    systemd.services = lib.mkMerge (lib.mapAttrsToList (_: ic:
      ic.relayService
    ) instanceConfigs);

    systemd.sockets = lib.mkMerge (lib.mapAttrsToList (_: ic:
      ic.socket
    ) instanceConfigs);
  };
}
