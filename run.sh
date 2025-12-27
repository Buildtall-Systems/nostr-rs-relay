#!/usr/bin/env bash

set -e

echo "Checking if binaries are built..."

# Check if auth server binary exists
if [ ! -f "./go-nip42-authz/nip42-authz" ]; then
    echo "Building NIP-42 authorization server..."
    cd go-nip42-authz && go build -o nip42-authz main.go && cd ..
fi

# Check if relay binary exists
if [ ! -f "./target/release/nostr-rs-relay" ]; then
    echo "Building relay..."
    cargo build --release
fi

echo "Starting auth server and relay..."

# Trap to kill all background processes on exit
cleanup() {
    echo "Shutting down..."
    trap - EXIT INT TERM  # Disable trap to prevent infinite loop
    kill 0
}
trap cleanup EXIT INT TERM

# Start auth server in background (from its directory so it finds config)
cd go-nip42-authz
./nip42-authz &
AUTH_PID=$!
cd ..
echo "Auth server started (PID: $AUTH_PID)"

# Give auth server a moment to start
sleep 1

# Start relay in foreground
echo "Starting relay..."
./target/release/nostr-rs-relay
