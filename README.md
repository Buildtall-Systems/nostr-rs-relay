# nostr-rs-relay

A [Nostr](https://github.com/nostr-protocol/nostr) relay written in Rust. This is a [buildtall.systems](https://buildtall.systems) fork of [nostr-rs-relay](https://git.sr.ht/~gheartsfield/nostr-rs-relay) by Greg Heartsfield.

## What Upstream Provides

The upstream relay is a complete NIP-compliant WebSocket relay with SQLite persistence, rate limiting, event size limits, NIP-42 authentication, and gRPC event admission hooks. It handles the core Nostr protocol:

### NIP Support (upstream)

- [x] NIP-01: [Basic protocol flow description](https://github.com/nostr-protocol/nips/blob/master/01.md)
- [x] NIP-02: [Contact List and Petnames](https://github.com/nostr-protocol/nips/blob/master/02.md)
- [x] NIP-05: [Mapping Nostr keys to DNS-based internet identifiers](https://github.com/nostr-protocol/nips/blob/master/05.md)
- [x] NIP-09: [Event Deletion](https://github.com/nostr-protocol/nips/blob/master/09.md)
- [x] NIP-11: [Relay Information Document](https://github.com/nostr-protocol/nips/blob/master/11.md)
- [x] NIP-12: [Generic Tag Queries](https://github.com/nostr-protocol/nips/blob/master/12.md)
- [x] NIP-15: [End of Stored Events Notice](https://github.com/nostr-protocol/nips/blob/master/15.md)
- [x] NIP-16: [Event Treatment](https://github.com/nostr-protocol/nips/blob/master/16.md)
- [x] NIP-20: [Command Results](https://github.com/nostr-protocol/nips/blob/master/20.md)
- [x] NIP-22: [Event `created_at` limits](https://github.com/nostr-protocol/nips/blob/master/22.md) (future-dated events only)
- [x] NIP-28: [Public Chat](https://github.com/nostr-protocol/nips/blob/master/28.md)
- [x] NIP-33: [Parameterized Replaceable Events](https://github.com/nostr-protocol/nips/blob/master/33.md)
- [x] NIP-40: [Expiration Timestamp](https://github.com/nostr-protocol/nips/blob/master/40.md)
- [x] NIP-42: [Authentication of clients to relays](https://github.com/nostr-protocol/nips/blob/master/42.md)

Upstream also provides the gRPC event admission interface (`nauthz.proto`) — an extensibility hook that allows external programs to approve or reject incoming events. See [gRPC Extensions](docs/grpc-extensions.md) for the upstream design document.

## What Buildtall Adds

Everything below is buildtall.systems work on top of the upstream relay.

### gRPC Relay Service

A tonic-based gRPC server exposes the full relay protocol for machine-to-machine communication. Defined in `proto/relay.proto` under the `nostr.relay.v1.Relay` service:

| RPC | Description |
|-----|-------------|
| `Publish` | Submit a signed event |
| `Subscribe` | Server-streaming subscription with EOSE |
| `Unsubscribe` | Close a subscription |
| `Auth` | NIP-42 authentication (kind 22242) |
| `Query` | One-shot batch query (no subscription) |

Enable by setting `relay_server_address` in `config.toml`:

```toml
[grpc]
relay_server_address = "[::1]:50053"
```

Disabled by default. See [gRPC Extensions](docs/grpc-extensions.md) for design details.

### relay-authz Integration

The upstream relay provides a generic gRPC event admission hook. Buildtall wires this to relay-authz, our authorization sidecar that manages trust tiers and write access. The NixOS module (below) manages relay-authz as a companion service automatically — generating its config, seeding admin npubs, and configuring the gRPC connection between relay and sidecar.

```toml
[grpc]
event_admission_server = "http://[::1]:50051"
restricts_write = true
```

### Systemd Socket Activation

Zero-downtime restarts via systemd socket passing. When enabled, systemd holds the listening socket and passes it to the relay on startup. During `systemctl restart`, new connections queue in the kernel TCP backlog while the relay restarts.

The relay:
- Accepts sockets from systemd via `LISTEN_FDS` (listenfd crate)
- Sends `READY=1` with actual listen address to systemd on startup
- Sends `STOPPING=1` on graceful shutdown
- Falls back to standard bind if no systemd socket is present

### Nix Packaging and NixOS Module

Buildtall adds a Nix flake (`flake.nix`) that builds the relay with crane and provides a declarative NixOS module for production deployment:

```nix
{
  services.nostr-relay = {
    enable = true;

    settings = {
      info = {
        relay_url = "wss://relay.example.com/";
        name = "My Relay";
      };
      network = {
        address = "127.0.0.1";
        port = 7777;
      };
      authorization.nip42_auth = true;
    };

    # Socket activation for zero-downtime restarts
    socketActivation = {
      enable = true;
      listenAddress = "0.0.0.0";
      port = 8080;
    };

    # Optional: relay-authz sidecar
    authz = {
      enable = true;
      grpcAddress = "[::1]:50051";
      adminNpubs = [ "npub1..." ];
    };
  };
}
```

The module manages:
- System user/group creation
- TOML config generation from `settings`
- systemd service (`Type=notify`) with hardening
- Optional socket unit for zero-downtime restarts
- Optional relay-authz sidecar with config generation

## Building and Running

### With Nix (recommended)

```console
$ nix build
$ ./result/bin/nostr-rs-relay --config config.toml
```

### From Source

Requires Rust, protobuf compiler, and pkg-config:

```console
$ nix develop  # or: apt install build-essential cmake protobuf-compiler pkg-config libssl-dev
$ cargo build --release
$ RUST_LOG=warn,nostr_rs_relay=info ./target/release/nostr-rs-relay -c config.toml
```

## Configuration

See [`config.toml`](config.toml) for all available options. Key sections:

| Section | Purpose |
|---------|---------|
| `[info]` | Relay metadata (NIP-11) |
| `[database]` | SQLite data directory |
| `[network]` | Bind address, port, remote IP header |
| `[grpc]` | Event admission server, gRPC relay server |
| `[authorization]` | NIP-42 auth settings |
| `[limits]` | Rate limiting, event size, connection limits |
| `[logging]` | Log file path and prefix |

## Reverse Proxy

See [Reverse Proxy](docs/reverse-proxy.md) for nginx/Caddy configuration examples.

## License

MIT. Original work by [Greg Heartsfield](https://git.sr.ht/~gheartsfield/nostr-rs-relay).
