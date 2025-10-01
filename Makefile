.PHONY: proto build-go build-rust build run-auth run-relay run clean

proto:
	nix-shell -p protobuf go protoc-gen-go protoc-gen-go-grpc --run "protoc --go_out=go-nip42-authz --go_opt=paths=source_relative --go_opt=Mproto/nauthz.proto=. --go-grpc_out=go-nip42-authz --go-grpc_opt=paths=source_relative --go-grpc_opt=Mproto/nauthz.proto=. proto/nauthz.proto"

build-go: proto
	cd go-nip42-authz && go build -o nip42-authz main.go

build-rust:
	nix-shell -p protobuf --run "cargo build --release"

build: build-go build-rust

run-auth: build-go
	./go-nip42-authz/nip42-authz

run-relay: build-rust
	./target/release/nostr-rs-relay

run:
	@echo "Starting auth server and relay..."
	@trap 'kill 0' EXIT; \
	./go-nip42-authz/nip42-authz & \
	./target/release/nostr-rs-relay

clean:
	rm -f go-nip42-authz/*.pb.go
	rm -f go-nip42-authz/nip42-authz
	cargo clean
