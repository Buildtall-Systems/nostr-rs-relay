.PHONY: build run clean

build:
	nix-shell -p protobuf --run "cargo build --release"

run: build
	./target/release/nostr-rs-relay

clean:
	cargo clean
