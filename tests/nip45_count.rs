use anyhow::{anyhow, Result};
use bitcoin_hashes::{sha256, Hash};
use futures::SinkExt;
use futures::StreamExt;
use secp256k1::{KeyPair, Message, Secp256k1, XOnlyPublicKey};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
mod common;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const SECKEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECKEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn pubkey_hex(seckey_hex: &str) -> Result<String> {
    let secp = Secp256k1::new();
    let keypair = KeyPair::from_seckey_str(&secp, seckey_hex)?;
    Ok(hex::encode(XOnlyPublicKey::from_keypair(&keypair).serialize()))
}

/// Build a signed EVENT message; returns (event id, wire message).
fn signed_event(
    seckey_hex: &str,
    created_at: u64,
    kind: u64,
    content: &str,
) -> Result<(String, String)> {
    let secp = Secp256k1::new();
    let keypair = KeyPair::from_seckey_str(&secp, seckey_hex)?;
    let pubkey = hex::encode(XOnlyPublicKey::from_keypair(&keypair).serialize());
    let tags = json!([]);
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

async fn publish(ws: &mut Ws, event_id: &str, raw: &str) -> Result<()> {
    ws.send(raw.to_owned().into()).await?;
    let v = recv_msg(ws).await?;
    if v[0] != "OK" || v[1] != event_id || v[2] != true {
        return Err(anyhow!("publish not accepted: {v}"));
    }
    Ok(())
}

/// Read messages until the COUNT response for the query id arrives.
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

/// Read messages until a CLOSED for the query id arrives; returns the reason.
async fn recv_closed(ws: &mut Ws, sub_id: &str) -> Result<String> {
    loop {
        let v = recv_msg(ws).await?;
        if v[0] == "CLOSED" && v[1] == sub_id {
            return v[2]
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("CLOSED without reason: {v}"));
        }
    }
}

#[tokio::test]
async fn nip45_count() -> Result<()> {
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;
    let (mut ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;

    // corpus: three kind-1 events by A, two by B.
    let a1 = signed_event(SECKEY_A, 1000, 1, "a1")?;
    let a2 = signed_event(SECKEY_A, 1001, 1, "a2")?;
    let a3 = signed_event(SECKEY_A, 1002, 1, "a3")?;
    let b1 = signed_event(SECKEY_B, 1003, 1, "b1")?;
    let b2 = signed_event(SECKEY_B, 1004, 1, "b2")?;
    for (id, raw) in [&a1, &a2, &a3, &b1, &b2] {
        publish(&mut ws, id, raw).await?;
    }
    let pub_a = pubkey_hex(SECKEY_A)?;

    // (a) author-scoped count: the storefront's exact filter shape.
    ws.send(
        json!(["COUNT", "c-author", {"kinds": [1], "authors": [pub_a]}])
            .to_string()
            .into(),
    )
    .await?;
    assert_eq!(recv_count(&mut ws, "c-author").await?, 3);

    // (b) overlapping filters aggregate as a deduplicated union: the
    // author filter's three events all also match the kind filter; a
    // per-filter sum would say 8.
    ws.send(
        json!(["COUNT", "c-union", {"authors": [pub_a]}, {"kinds": [1]}])
            .to_string()
            .into(),
    )
    .await?;
    assert_eq!(recv_count(&mut ws, "c-union").await?, 5);

    // (c) a filter limit bounds the count.
    ws.send(json!(["COUNT", "c-bounded", {"kinds": [1], "limit": 2}]).to_string().into())
        .await?;
    assert_eq!(recv_count(&mut ws, "c-bounded").await?, 2);

    // (d) no matches counts zero.
    ws.send(json!(["COUNT", "c-zero", {"kinds": [30023]}]).to_string().into())
        .await?;
    assert_eq!(recv_count(&mut ws, "c-zero").await?, 0);

    // (e) a filter refused by extension validation answers CLOSED:
    // until_id without order is invalid.
    ws.send(
        json!(["COUNT", "c-invalid", {"kinds": [1], "until": 2000, "until_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}])
            .to_string()
            .into(),
    )
    .await?;
    let reason = recv_closed(&mut ws, "c-invalid").await?;
    assert!(reason.starts_with("invalid:"), "unexpected CLOSED reason: {reason}");

    // (f) NIP-11 advertises 45.
    let req = hyper::Request::builder()
        .uri(format!("http://127.0.0.1:{}/", relay.port))
        .header("Accept", "application/nostr+json")
        .body(hyper::Body::empty())?;
    let res = hyper::Client::new().request(req).await?;
    let body = hyper::body::to_bytes(res.into_body()).await?;
    let info: Value = serde_json::from_slice(&body)?;
    let nips = info["supported_nips"]
        .as_array()
        .ok_or_else(|| anyhow!("no supported_nips in relay info: {info}"))?;
    assert!(nips.contains(&json!(45)), "45 missing from {nips:?}");

    relay.shutdown_tx.send(()).ok();
    Ok(())
}
