.PHONY: build run clean lint test

NIXSYS := $(shell nix eval --impure --raw --expr builtins.currentSystem)

build:
	nix build .#nostr-rs-relay

run: build
	./result/bin/nostr-rs-relay

lint:
	nix build .#checks.$(NIXSYS).clippy --no-link

test:
	nix build .#checks.$(NIXSYS).crate --no-link

clean:
	cargo clean
	rm -f result
