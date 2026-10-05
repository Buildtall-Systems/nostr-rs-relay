use anyhow::{anyhow, Result};
use bitcoin_hashes::{sha256, Hash};
use futures::SinkExt;
use futures::StreamExt;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use secp256k1::{KeyPair, Message, Secp256k1, XOnlyPublicKey};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::mpsc as syncmpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
mod common;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const SECKEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECKEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RELAY_URL: &str = "ws://relay.test/";

fn pubkey_hex(seckey_hex: &str) -> Result<String> {
    let secp = Secp256k1::new();
    let keypair = KeyPair::from_seckey_str(&secp, seckey_hex)?;
    Ok(hex::encode(XOnlyPublicKey::from_keypair(&keypair).serialize()))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build a signed event object; returns (event id, event JSON).
fn signed(
    seckey_hex: &str,
    created_at: u64,
    kind: u64,
    tags: Value,
    content: &str,
) -> Result<(String, Value)> {
    let secp = Secp256k1::new();
    let keypair = KeyPair::from_seckey_str(&secp, seckey_hex)?;
    let pubkey = hex::encode(XOnlyPublicKey::from_keypair(&keypair).serialize());
    let canonical = json!([0, pubkey, created_at, kind, tags, content]).to_string();
    let digest: sha256::Hash = sha256::Hash::hash(canonical.as_bytes());
    let id = format!("{digest:x}");
    let msg = Message::from_slice(digest.as_ref())?;
    let sig = secp.sign_schnorr(&msg, &keypair);
    let event = json!({
        "id": id,
        "pubkey": pubkey,
        "created_at": created_at,
        "kind": kind,
        "tags": tags,
        "content": content,
        "sig": sig.to_string(),
    });
    Ok((id, event))
}

async fn recv_msg(ws: &mut Ws) -> Result<Value> {
    loop {
        let msg = timeout(Duration::from_secs(10), ws.next())
            .await?
            .ok_or_else(|| anyhow!("websocket closed"))??;
        if msg.is_text() {
            return Ok(serde_json::from_str(msg.to_text()?)?);
        }
    }
}

/// Connect and consume the NIP-42 challenge the relay sends first.
async fn connect(port: u16) -> Result<(Ws, String)> {
    let (mut ws, _res) = connect_async(format!("ws://localhost:{port}")).await?;
    let v = recv_msg(&mut ws).await?;
    if v[0] != "AUTH" {
        return Err(anyhow!("expected an AUTH challenge first, got {v}"));
    }
    let challenge = v[1]
        .as_str()
        .ok_or_else(|| anyhow!("AUTH without challenge: {v}"))?;
    Ok((ws, challenge.to_owned()))
}

/// Send a message and wait for the OK for `event_id`.
async fn expect_ok(ws: &mut Ws, raw: String, event_id: &str) -> Result<()> {
    ws.send(raw.into()).await?;
    loop {
        let v = recv_msg(ws).await?;
        if v[0] == "OK" && v[1] == event_id {
            if v[2] != true {
                return Err(anyhow!("not accepted: {v}"));
            }
            return Ok(());
        }
    }
}

async fn publish(ws: &mut Ws, seckey: &str, kind: u64, content: &str) -> Result<String> {
    let (id, event) = signed(seckey, now(), kind, json!([]), content)?;
    expect_ok(ws, json!(["EVENT", event]).to_string(), &id).await?;
    Ok(id)
}

async fn authenticate(ws: &mut Ws, seckey: &str, challenge: &str) -> Result<()> {
    let tags = json!([["relay", RELAY_URL], ["challenge", challenge]]);
    let (id, event) = signed(seckey, now(), 22242, tags, "")?;
    expect_ok(ws, json!(["AUTH", event]).to_string(), &id).await
}

/// Read until CLOSED for `sub_id`; returns the reason.
async fn recv_closed(ws: &mut Ws, sub_id: &str) -> Result<String> {
    loop {
        let v = recv_msg(ws).await?;
        if v[0] == "EVENT" && v[1] == sub_id {
            return Err(anyhow!("EVENT sent before CLOSED: {v}"));
        }
        if v[0] == "CLOSED" && v[1] == sub_id {
            return v[2]
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("CLOSED without reason: {v}"));
        }
    }
}

/// Read until EOSE for `sub_id`; returns the ids of the stored events.
async fn recv_stored(ws: &mut Ws, sub_id: &str) -> Result<HashSet<String>> {
    let mut ids = HashSet::new();
    loop {
        let v = recv_msg(ws).await?;
        if v[1] != sub_id {
            continue;
        }
        if v[0] == "EOSE" {
            return Ok(ids);
        }
        if v[0] == "EVENT" {
            let id = v[2]["id"]
                .as_str()
                .ok_or_else(|| anyhow!("EVENT without id: {v}"))?;
            ids.insert(id.to_owned());
        }
    }
}

async fn recv_count(ws: &mut Ws, sub_id: &str) -> Result<u64> {
    loop {
        let v = recv_msg(ws).await?;
        if v[0] == "COUNT" && v[1] == sub_id {
            return v[2]["count"]
                .as_u64()
                .ok_or_else(|| anyhow!("COUNT response without count: {v}"));
        }
    }
}

// Relays in one test binary share an in-memory database, and concurrent
// writers lock it, so the scenarios run in sequence.
#[tokio::test]
async fn author_only_reads() -> Result<()> {
    private_relay().await?;
    default_relay().await
}

async fn private_relay() -> Result<()> {
    let relay = common::start_relay_with(|s| {
        s.info.relay_url = Some(RELAY_URL.to_owned());
        s.authorization.nip42_auth = true;
        s.authorization.author_only_reads = true;
    })?;
    common::wait_for_healthy_relay(&relay).await?;
    let pub_b = pubkey_hex(SECKEY_B)?;

    // writes are not gated by this setting: publish a corpus from an
    // unauthenticated connection.
    let (mut ws, challenge) = connect(relay.port).await?;
    let a1 = publish(&mut ws, SECKEY_A, 11, "a1").await?;
    let a2 = publish(&mut ws, SECKEY_A, 11, "a2").await?;
    let b1 = publish(&mut ws, SECKEY_B, 11, "b1").await?;

    // (a) unauthenticated REQ and COUNT are closed with auth-required.
    ws.send(json!(["REQ", "anon", {"kinds": [11]}]).to_string().into())
        .await?;
    let reason = recv_closed(&mut ws, "anon").await?;
    assert!(reason.starts_with("auth-required:"), "unexpected reason: {reason}");
    ws.send(json!(["COUNT", "anon-count", {"kinds": [11]}]).to_string().into())
        .await?;
    let reason = recv_closed(&mut ws, "anon-count").await?;
    assert!(reason.starts_with("auth-required:"), "unexpected reason: {reason}");

    authenticate(&mut ws, SECKEY_A, &challenge).await?;

    // (b) a broad request returns only A's events.
    ws.send(json!(["REQ", "all", {"kinds": [11]}]).to_string().into())
        .await?;
    let got = recv_stored(&mut ws, "all").await?;
    assert_eq!(got, HashSet::from([a1.clone(), a2.clone()]));

    // (c) naming B, by author or by id, returns nothing.
    ws.send(json!(["REQ", "by-author", {"authors": [pub_b]}]).to_string().into())
        .await?;
    assert!(recv_stored(&mut ws, "by-author").await?.is_empty());
    ws.send(json!(["REQ", "by-id", {"ids": [b1]}]).to_string().into())
        .await?;
    assert!(recv_stored(&mut ws, "by-id").await?.is_empty());

    // (d) COUNT counts only A's events.
    ws.send(json!(["COUNT", "count", {"kinds": [11]}]).to_string().into())
        .await?;
    assert_eq!(recv_count(&mut ws, "count").await?, 2);

    // (e) live events: B's never arrives, A's does. B publishes first,
    // so a leak would arrive before A's event.
    ws.send(json!(["REQ", "live", {"kinds": [7]}]).to_string().into())
        .await?;
    assert!(recv_stored(&mut ws, "live").await?.is_empty());
    let (mut writer, _) = connect(relay.port).await?;
    publish(&mut writer, SECKEY_B, 7, "b-live").await?;
    let a_live = publish(&mut writer, SECKEY_A, 7, "a-live").await?;
    loop {
        let v = recv_msg(&mut ws).await?;
        if v[0] == "EVENT" && v[1] == "live" {
            assert_eq!(v[2]["id"], json!(a_live), "unexpected live event: {v}");
            break;
        }
    }

    // (f) NIP-11 advertises that reads need AUTH.
    let req = hyper::Request::builder()
        .uri(format!("http://127.0.0.1:{}/", relay.port))
        .header("Accept", "application/nostr+json")
        .body(hyper::Body::empty())?;
    let res = hyper::Client::new().request(req).await?;
    let body = hyper::body::to_bytes(res.into_body()).await?;
    let info: Value = serde_json::from_slice(&body)?;
    assert_eq!(info["limitation"]["auth_required"], json!(true), "{info}");

    relay.shutdown_tx.send(()).ok();
    relay
        .handle
        .join()
        .map_err(|_| anyhow!("relay thread panicked"))?;
    Ok(())
}

async fn default_relay() -> Result<()> {
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;
    let (mut ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;
    let b1 = publish(&mut ws, SECKEY_B, 1, "b1").await?;

    // without author_only_reads, an unauthenticated client reads any
    // author's events. The database may still hold the private relay's
    // corpus, so the request names the event rather than a kind.
    ws.send(json!(["REQ", "by-id", {"ids": [b1]}]).to_string().into())
        .await?;
    assert_eq!(recv_stored(&mut ws, "by-id").await?, HashSet::from([b1]));

    relay.shutdown_tx.send(()).ok();
    relay
        .handle
        .join()
        .map_err(|_| anyhow!("relay thread panicked"))?;
    Ok(())
}

#[test]
fn author_only_reads_requires_nip42_auth() {
    let mut settings = config::Settings::default();
    settings.info.relay_url = Some(RELAY_URL.to_owned());
    settings.authorization.author_only_reads = true;
    let (_tx, rx) = syncmpsc::channel();
    assert!(start_server(&settings, rx).is_err());
}

#[test]
fn author_only_reads_requires_relay_url() {
    let mut settings = config::Settings::default();
    settings.authorization.nip42_auth = true;
    settings.authorization.author_only_reads = true;
    let (_tx, rx) = syncmpsc::channel();
    assert!(start_server(&settings, rx).is_err());
}
