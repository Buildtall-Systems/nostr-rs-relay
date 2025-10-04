#!/usr/bin/env bash
set -e

HOST=nostr.io
USER=nostr
DEPLOY_DIR=/home/nostr/relay/relay.drss.io/deploy

make build-for-deploy

scp ./target/release/nostr-rs-relay $USER@$HOST:$DEPLOY_DIR/
scp ./go-nip42-authz/nip42-authz $USER@$HOST:$DEPLOY_DIR/
scp ./config.prod.toml $USER@$HOST:$DEPLOY_DIR/config.toml
scp ./go-nip42-authz/policy-config.prod.toml $USER@$HOST:$DEPLOY_DIR/policy-config.toml
scp ./services/relay.drss.io.service $USER@$HOST:$DEPLOY_DIR/
scp ./services/nip42-authz.service $USER@$HOST:$DEPLOY_DIR/
scp ./setup.sh $USER@$HOST:$DEPLOY_DIR/

