#!/usr/bin/env bash

systemctl stop relay.drss.io || true
systemctl stop relay.buildtall.com || true
systemctl stop nip42-authz || true

trap 'systemctl start relay.drss.io; systemctl start relay.buildtall.com; systemctl start nip42-authz' EXIT

set -e

cp /home/nostr/relay/relay.drss.io/deploy/nostr-rs-relay /usr/local/bin/
cp /home/nostr/relay/relay.drss.io/deploy/nip42-authz /usr/local/bin/
chown relay:relay /usr/local/bin/nostr-rs-relay
chown relay:relay /usr/local/bin/nip42-authz

cp /home/nostr/relay/relay.drss.io/deploy/config.toml /var/lib/relay.drss.io/
cp /home/nostr/relay/relay.drss.io/deploy/policy-config.toml /var/lib/relay.drss.io/
chown relay:relay /var/lib/relay.drss.io/config.toml
chown relay:relay /var/lib/relay.drss.io/policy-config.toml
