# Implementation Plan: NixOS Deployable Module for nostr-rs-relay + go-nip42-authz

## Overview

**Goal**: Transform the existing flake into a deployable NixOS module that packages both the Rust relay and Go authorization sidecar, with unified declarative configuration.

**Approach**: Extend the flake to output both packages and a NixOS module. The module will generate TOML configs from Nix attribute sets and manage systemd services with proper ordering.

## Architecture

```
flake.nix
├── packages.nostr-rs-relay      # Rust relay binary (existing, enhanced)
├── packages.nip42-authz         # Go authz binary (NEW)
├── packages.default             # nostr-rs-relay (unchanged)
└── nixosModules.default         # NixOS service module (NEW)

NixOS Module (services.nostr-relay)
├── Main relay service (systemd)
│   └── config.toml (generated from Nix options)
└── Authz sidecar service (optional, systemd)
    └── policy-config.toml (generated from Nix options)
```

## Phase 1: Add Go Package for nip42-authz

**Files to modify:**
- `flake.nix` - Add buildGoModule for the Go authz service
- `Makefile` - Add `vendor` target to update vendor directory
- `go-nip42-authz/vendor/` - Create vendor directory with dependencies

**Changes:**

1. **Create vendor directory** (one-time setup):
   ```bash
   cd go-nip42-authz
   go mod vendor
   ```

2. **Add Makefile target** for updating vendor:
   ```makefile
   .PHONY: vendor
   vendor:
   	cd go-nip42-authz && go mod vendor
   ```

3. Add `pkgs.buildGoModule` derivation for `go-nip42-authz`:
   ```nix
   nip42-authz = pkgs.buildGoModule {
     pname = "nip42-authz";
     version = "0.1.0";
     src = ./go-nip42-authz;

     # Use vendored dependencies (no hash needed)
     vendorHash = null;

     # Proto files are pre-generated, no preBuild needed

     ldflags = [ "-s" "-w" ];

     meta = {
       description = "NIP-42 Authorization gRPC service for nostr-rs-relay";
       license = lib.licenses.mit;
     };
   };
   ```

4. Export in `packages`:
   ```nix
   packages = {
     default = crate;
     nostr-rs-relay = crate;
     nip42-authz = nip42-authz;
   };
   ```

**Verification:**
- `nix build .#nip42-authz` succeeds
- Binary runs: `./result/bin/nip42-authz --help` or starts (may fail without config, that's OK)

**Maintenance:**
- When Go dependencies change, run `make vendor` to update the vendor directory
- Commit the updated vendor directory to git

## Phase 2: Create NixOS Module Structure

**Files to create:**
- `nix/module.nix` - The NixOS module definition

**Module Option Structure:**

```nix
options.services.nostr-relay = {
  enable = mkEnableOption "nostr-rs-relay Nostr relay server";

  package = mkOption {
    type = types.package;
    default = self.packages.${system}.nostr-rs-relay;
    description = "The nostr-rs-relay package to use";
  };

  user = mkOption {
    type = types.str;
    default = "nostr";
    description = "User under which the relay runs";
  };

  group = mkOption {
    type = types.str;
    default = "nostr";
    description = "Group under which the relay runs";
  };

  dataDir = mkOption {
    type = types.path;
    default = "/var/lib/nostr-relay";
    description = "Directory for relay data (SQLite database)";
  };

  settings = mkOption {
    type = types.submodule { ... };  # Structured relay config
    default = {};
    description = "Relay configuration (generates config.toml)";
  };

  authz = {
    enable = mkEnableOption "NIP-42 authorization sidecar";

    package = mkOption {
      type = types.package;
      default = self.packages.${system}.nip42-authz;
    };

    listenAddress = mkOption {
      type = types.str;
      default = "[::1]:50051";
      description = "gRPC listen address for authz service";
    };

    logLevel = mkOption {
      type = types.enum [ "DEBUG" "INFO" "WARN" "ERROR" ];
      default = "ERROR";
    };

    allowedNpubs = mkOption {
      type = types.listOf types.str;
      default = [];
      description = "List of npubs allowed to publish";
    };
  };
};
```

**Settings Submodule (relay config):**

The `settings` option should mirror the TOML structure. Key sections:

```nix
settings = mkOption {
  type = types.submodule {
    options = {
      info = {
        relay_url = mkOption { type = types.str; };
        name = mkOption { type = types.str; };
        description = mkOption { type = types.str; default = ""; };
        pubkey = mkOption { type = types.nullOr types.str; default = null; };
        contact = mkOption { type = types.nullOr types.str; default = null; };
      };

      database = {
        engine = mkOption { type = types.enum [ "sqlite" "postgres" ]; default = "sqlite"; };
        data_directory = mkOption { type = types.nullOr types.path; default = null; };
        # ... other database options
      };

      network = {
        address = mkOption { type = types.str; default = "0.0.0.0"; };
        port = mkOption { type = types.port; default = 7777; };
      };

      grpc = {
        event_admission_server = mkOption { type = types.nullOr types.str; default = null; };
        restricts_write = mkOption { type = types.bool; default = false; };
      };

      authorization = {
        nip42_auth = mkOption { type = types.bool; default = false; };
        pubkey_whitelist = mkOption { type = types.nullOr (types.listOf types.str); default = null; };
      };

      limits = { /* rate limiting options */ };
      options = { /* misc options */ };
      # ... other sections as needed
    };
  };
};
```

**Alternative: Freeform Settings**

For flexibility, could use freeform TOML generation:

```nix
settings = mkOption {
  type = types.attrsOf types.anything;
  default = {};
  description = "Relay settings (converted to TOML)";
  example = {
    info = { relay_url = "wss://relay.example.com/"; name = "My Relay"; };
    network = { port = 7777; };
  };
};
```

This is simpler but loses type checking. **Recommendation**: Start with freeform for MVP, add typed options later if needed.

## Phase 3: Implement Config Generation

**In `nix/module.nix`:**

1. **Generate relay config.toml:**
   ```nix
   relayConfig = pkgs.writeText "config.toml" (lib.generators.toTOML {} (
     lib.recursiveUpdate {
       # Defaults
       database.data_directory = cfg.dataDir;
       grpc = lib.optionalAttrs cfg.authz.enable {
         event_admission_server = "http://${cfg.authz.listenAddress}";
         restricts_write = true;
       };
     } cfg.settings
   ));
   ```

2. **Generate authz policy-config.toml:**
   ```nix
   authzConfig = pkgs.writeText "policy-config.toml" (lib.generators.toTOML {} {
     log_level = cfg.authz.logLevel;
     listen_address = cfg.authz.listenAddress;
     allowed_npubs = cfg.authz.allowedNpubs;
   });
   ```

**Verification:**
- Nix evaluation succeeds
- Generated TOML is valid and matches expected structure

## Phase 4: Implement Systemd Services

**In `nix/module.nix`:**

1. **Create user/group:**
   ```nix
   users.users.${cfg.user} = {
     isSystemUser = true;
     group = cfg.group;
     home = cfg.dataDir;
     createHome = true;
   };
   users.groups.${cfg.group} = {};
   ```

2. **Authz service (if enabled):**
   ```nix
   systemd.services.nip42-authz = lib.mkIf cfg.authz.enable {
     description = "NIP-42 Authorization Service";
     wantedBy = [ "multi-user.target" ];
     before = [ "nostr-relay.service" ];  # Start before relay

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
     };

     environment = {
       # The Go app reads config from CWD, so we need to place it there
       # OR modify the app to accept --config flag
     };

     preStart = ''
       # Copy config to working directory
       cp ${authzConfig} ${cfg.dataDir}/policy-config.toml
     '';
   };
   ```

3. **Relay service:**
   ```nix
   systemd.services.nostr-relay = lib.mkIf cfg.enable {
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
   ```

**Note on authz config path**: The Go app currently looks for `policy-config.toml` in CWD. Options:
1. Copy config to dataDir in preStart (simple, chosen above)
2. Modify Go app to accept `--config` flag (better long-term)
3. Set working directory to a temp location with symlinked config

## Phase 5: Wire Module into Flake

**Modify `flake.nix`:**

```nix
outputs = inputs@{ self, ... }:
  (inputs.flake-utils.lib.eachDefaultSystem (system:
    # ... existing per-system outputs ...
  )) // {
    # System-independent outputs
    nixosModules.default = import ./nix/module.nix { inherit self; };
    nixosModules.nostr-relay = self.nixosModules.default;
  };
```

The module needs access to `self` to reference the packages. This requires a pattern like:

```nix
# nix/module.nix
{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.nostr-relay;

  # Get packages for the current system
  relayPkg = self.packages.${pkgs.system}.nostr-rs-relay;
  authzPkg = self.packages.${pkgs.system}.nip42-authz;
in
{
  options.services.nostr-relay = { ... };

  config = lib.mkIf cfg.enable { ... };
}
```

## Phase 6: Example Configuration & Documentation

**Create `nix/example.nix`:**

```nix
# Example NixOS configuration using the relay module
{ config, pkgs, ... }:

{
  imports = [
    # Import from flake: inputs.nostr-rs-relay.nixosModules.default
  ];

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
      };

      network = {
        address = "127.0.0.1";  # Behind reverse proxy
        port = 7777;
      };

      authorization = {
        nip42_auth = true;
      };
    };

    authz = {
      enable = true;
      logLevel = "INFO";
      allowedNpubs = [
        "npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx"
      ];
    };
  };
}
```

**Update README** with:
- How to add flake input
- How to import and configure the module
- Example configurations (minimal, with authz, production)

## File Summary

| File | Action | Description |
|------|--------|-------------|
| `flake.nix` | MODIFY | Add nip42-authz package, add nixosModules output |
| `Makefile` | MODIFY | Add `vendor` target to update Go vendor directory |
| `go-nip42-authz/vendor/` | CREATE | Vendored Go dependencies (run `go mod vendor`) |
| `nix/module.nix` | CREATE | NixOS module with options, config generation, systemd services |
| `nix/example.nix` | CREATE | Example configuration for documentation |
| `README.md` | MODIFY | Add NixOS deployment documentation |

## Implementation Order

1. **Phase 1**: Add Go package to flake
   - Verify: `nix build .#nip42-authz`

2. **Phase 2-3**: Create module structure with options and config generation
   - Verify: Nix evaluation succeeds, inspect generated TOML

3. **Phase 4**: Add systemd services
   - Verify: Module evaluates, services defined correctly

4. **Phase 5**: Wire into flake outputs
   - Verify: `nix flake show` lists the module

5. **Phase 6**: Example and documentation
   - Verify: Example config evaluates without errors

## Testing Strategy

**Local testing (without full NixOS):**
```bash
# Build both packages
nix build .#nostr-rs-relay
nix build .#nip42-authz

# Verify flake outputs
nix flake show

# Check module evaluates (dry-run)
nix eval .#nixosModules.default --apply 'x: "module exists"'
```

**Full integration test (in NixOS VM or real system):**
```nix
# In a test configuration
services.nostr-relay = {
  enable = true;
  settings.info.relay_url = "ws://localhost:7777/";
  settings.network.port = 7777;
  authz.enable = true;
  authz.allowedNpubs = [ "npub1..." ];
};
```

Then verify:
- Both services start
- Relay connects to authz on localhost:50051
- Publishing with allowed npub succeeds
- Publishing with unknown npub is denied

## Rollback Plan

If issues arise:
- The existing flake.nix remains functional for the relay-only case
- Module can be disabled (`enable = false`)
- Authz can be disabled independently (`authz.enable = false`)
- Fall back to manual config files if generated ones have issues

## Open Questions / Future Enhancements

1. **Secrets management**: `allowedNpubs` in config is fine, but if we add features needing secrets (nsec for DMs, API keys), we'd need sops-nix or agenix integration.

2. **Config file path for authz**: Currently the Go app reads from CWD. Adding a `--config` flag would be cleaner.

3. **Health checks**: Add systemd health checks / watchdog for both services.

4. **Metrics**: nostr-rs-relay has Prometheus metrics. Could expose an option for metrics port.

5. **Reverse proxy integration**: Could add optional nginx/caddy config generation.
