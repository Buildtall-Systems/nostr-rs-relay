# nostr-rs-relay gRPC Transport Work Log

## 2026-03-23

### Phase 1: Proto Schema Definition — COMPLETE

**Merged**: Buildtall-Systems/nostr-rs-relay#1 to master

Proto schema fully defined per ADR Discussion #20:
- `proto/relay.proto` — 95-line service definition (Publish, Subscribe, Unsubscribe, Auth, Query)
- `build.rs` — Updated to compile relay.proto with tonic-build (server+client codegen)
- Type alignment: int64 created_at, int32 kind, string content per NIP-01 + go-nostr
- Verification: cargo build/test pass, review feedback resolved

**Next phase**: Phase 2 (Rust gRPC server implementation) — create src/grpc_server.rs, grpc_convert.rs, wire into start_server()

### Phase 2: Rust gRPC Server Implementation — COMPLETE

**Branch**: feature/grpc-server from develop

Implemented full tonic gRPC Relay service alongside WebSocket:
- `src/grpc_convert.rs` — Proto ↔ internal type converters (Event, Filter, Tag) with round-trip unit tests
- `src/grpc_server.rs` — Tonic Relay service impl: Publish (event pipeline + notice feedback), Subscribe (server-streaming: stored events → EOSE → live bcast), Unsubscribe, Auth (kind 22242), Query (one-shot batch)
- `src/config.rs` — Added `relay_server_address: Option<String>` to Grpc struct
- `src/server.rs` — Spawns tonic server on separate port if configured, shares event_tx/bcast_tx/repo/settings
- `Cargo.toml` — Added tokio-stream dependency
- `config.toml` — Documented relay_server_address option (disabled by default)
- Verification: cargo build --release ✅, cargo test --release ✅, cargo clippy (new code clean) ✅

**Key decisions**: Per-connection auth state keyed by peer address. Subscribe uses query_subscription for stored events + broadcast channel for live matching. Config-gated (disabled by default).

**Next phase**: Phase 3 (Go gRPC client library in btk/relay/grpc/)
