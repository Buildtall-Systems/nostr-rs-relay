use anyhow::{anyhow, Result};
use bitcoin_hashes::{sha256, Hash};
use futures::SinkExt;
use futures::StreamExt;
use secp256k1::{KeyPair, Message, Secp256k1, XOnlyPublicKey};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
mod common;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const SECKEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WIKI: u64 = 30818;
const LONG_FORM: u64 = 30023;

/// Build a signed EVENT message; returns (event id, wire message).
fn signed_event(
    created_at: u64,
    kind: u64,
    tags: Value,
    content: &str,
) -> Result<(String, String)> {
    let secp = Secp256k1::new();
    let keypair = KeyPair::from_seckey_str(&secp, SECKEY_A)?;
    let pubkey = hex::encode(XOnlyPublicKey::from_keypair(&keypair).serialize());
    let canonical = json!([0, pubkey, created_at, kind, tags, content]).to_string();
    let digest: sha256::Hash = sha256::Hash::hash(canonical.as_bytes());
    let id = format!("{digest:x}");
    let msg = Message::from_slice(digest.as_ref())?;
    let sig = secp.sign_schnorr(&msg, &keypair);
    let event = json!([
        "EVENT",
        {
            "id": id,
            "pubkey": pubkey,
            "created_at": created_at,
            "kind": kind,
            "tags": tags,
            "content": content,
            "sig": sig.to_string(),
        }
    ])
    .to_string();
    Ok((id, event))
}

/// A version of the page `d` at `created_at`.  Relays in one test
/// process share the in-memory database, so each test uses its own `d`.
fn version(kind: u64, d: &str, created_at: u64, content: &str) -> Result<(String, String)> {
    signed_event(created_at, kind, json!([["d", d]]), content)
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

/// Publish an event and return the OK message, which must accept it.
async fn publish(ws: &mut Ws, (id, raw): &(String, String)) -> Result<String> {
    ws.send(raw.clone().into()).await?;
    loop {
        let v = recv_msg(ws).await?;
        if v[0] == "OK" && v[1] == id.as_str() {
            if v[2] != true {
                return Err(anyhow!("publish not accepted: {v}"));
            }
            return Ok(v[3].as_str().unwrap_or_default().to_owned());
        }
    }
}

/// The ids of the stored versions of page `d` for `kind`.
async fn stored(ws: &mut Ws, kind: u64, d: &str) -> Result<HashSet<String>> {
    let sub_id = format!("versions-{kind}");
    let filter = json!({"kinds": [kind], "#d": [d]});
    ws.send(json!(["REQ", sub_id, filter]).to_string().into())
        .await?;
    let mut ids = HashSet::new();
    loop {
        let v = recv_msg(ws).await?;
        if v[1] != sub_id.as_str() {
            continue;
        }
        if v[0] == "EOSE" {
            ws.send(json!(["CLOSE", sub_id]).to_string().into()).await?;
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

fn ids(versions: &[&(String, String)]) -> HashSet<String> {
    versions.iter().map(|(id, _)| id.clone()).collect()
}

async fn connect(history_kinds: Vec<u64>) -> Result<(common::Relay, Ws)> {
    let relay = common::start_relay_with(|s| s.options.history_kinds = history_kinds)?;
    common::wait_for_healthy_relay(&relay).await?;
    let (ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;
    Ok((relay, ws))
}

#[tokio::test]
async fn history_kind_keeps_every_version() -> Result<()> {
    let (relay, mut ws) = connect(vec![WIKI]).await?;

    let v1 = version(WIKI, "kept", 1000, "first")?;
    let v2 = version(WIKI, "kept", 2000, "second")?;
    let v3 = version(WIKI, "kept", 3000, "third")?;
    // the oldest version arrives last, after the newer ones.
    for v in [&v2, &v3, &v1] {
        assert!(publish(&mut ws, v).await?.is_empty(), "not saved");
    }
    assert_eq!(stored(&mut ws, WIKI, "kept").await?, ids(&[&v1, &v2, &v3]));

    // a kind 5 deletion by e tag hides exactly the version it names.
    let deletion = signed_event(4000, 5, json!([["e", v2.0]]), "")?;
    publish(&mut ws, &deletion).await?;
    assert_eq!(stored(&mut ws, WIKI, "kept").await?, ids(&[&v1, &v3]));

    // a kind with the same shape outside the list keeps only its newest version.
    let l1 = version(LONG_FORM, "kept", 1000, "first")?;
    let l2 = version(LONG_FORM, "kept", 2000, "second")?;
    let l0 = version(LONG_FORM, "kept", 500, "older")?;
    publish(&mut ws, &l1).await?;
    publish(&mut ws, &l2).await?;
    assert!(publish(&mut ws, &l0).await?.starts_with("duplicate"));
    assert_eq!(stored(&mut ws, LONG_FORM, "kept").await?, ids(&[&l2]));

    relay.shutdown_tx.send(()).ok();
    Ok(())
}

#[tokio::test]
async fn without_the_setting_only_the_newest_version_stays() -> Result<()> {
    let (relay, mut ws) = connect(vec![]).await?;

    let v1 = version(WIKI, "replaced", 1000, "first")?;
    let v2 = version(WIKI, "replaced", 2000, "second")?;
    let v0 = version(WIKI, "replaced", 500, "older")?;
    publish(&mut ws, &v1).await?;
    publish(&mut ws, &v2).await?;
    assert!(publish(&mut ws, &v0).await?.starts_with("duplicate"));
    assert_eq!(stored(&mut ws, WIKI, "replaced").await?, ids(&[&v2]));

    relay.shutdown_tx.send(()).ok();
    Ok(())
}

#[test]
fn history_kinds_defaults_to_empty() {
    assert!(nostr_rs_relay::config::Settings::default()
        .options
        .history_kinds
        .is_empty());
}
