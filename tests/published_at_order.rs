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

/// Build a signed kind-1 EVENT message; returns (event id, wire message).
fn signed_event(
    seckey_hex: &str,
    created_at: u64,
    tags: Value,
    content: &str,
) -> Result<(String, String)> {
    let secp = Secp256k1::new();
    let keypair = KeyPair::from_seckey_str(&secp, seckey_hex)?;
    let pubkey = hex::encode(XOnlyPublicKey::from_keypair(&keypair).serialize());
    let canonical = json!([0, pubkey, created_at, 1, tags, content]).to_string();
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
            "kind": 1,
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

/// Read messages until EOSE for the subscription; returns event ids in
/// arrival order plus any NOTICE payloads seen along the way.
async fn collect_page(ws: &mut Ws, sub_id: &str) -> Result<(Vec<String>, Vec<String>)> {
    let mut events = vec![];
    let mut notices = vec![];
    loop {
        let v = recv_msg(ws).await?;
        match v[0].as_str() {
            Some("EVENT") if v[1] == sub_id => {
                events.push(
                    v[2]["id"]
                        .as_str()
                        .ok_or_else(|| anyhow!("event without id"))?
                        .to_owned(),
                );
            }
            Some("EOSE") if v[1] == sub_id => break,
            Some("NOTICE") => {
                notices.push(
                    v[1].as_str()
                        .ok_or_else(|| anyhow!("notice without text"))?
                        .to_owned(),
                );
            }
            _ => {}
        }
    }
    Ok((events, notices))
}

async fn publish(ws: &mut Ws, event_id: &str, raw: &str) -> Result<()> {
    ws.send(raw.to_owned().into()).await?;
    let v = recv_msg(ws).await?;
    if v[0] != "OK" || v[1] != event_id || v[2] != true {
        return Err(anyhow!("publish not accepted: {v}"));
    }
    Ok(())
}

#[tokio::test]
async fn published_at_order_extension() -> Result<()> {
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;
    let (mut ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;

    // corpus: created_at strictly increasing (import order), publication
    // times deliberately out of order, with a tie group at 600 wider
    // than the page size.  e7 has no published_at tag and e8 an
    // unparseable one; both fall back to created_at.
    let e1 = signed_event(SECKEY_A, 1000, json!([["published_at", "500"]]), "e1")?;
    let e2 = signed_event(SECKEY_B, 1001, json!([["published_at", "900"]]), "e2")?;
    let e3 = signed_event(SECKEY_A, 1002, json!([["published_at", "700"]]), "e3")?;
    let e4 = signed_event(SECKEY_A, 1003, json!([["published_at", "600"]]), "e4")?;
    let e5 = signed_event(SECKEY_B, 1004, json!([["published_at", "600"]]), "e5")?;
    let e6 = signed_event(SECKEY_A, 1005, json!([["published_at", "600"]]), "e6")?;
    let e7 = signed_event(SECKEY_B, 1006, json!([]), "e7")?;
    let e8 = signed_event(SECKEY_A, 1007, json!([["published_at", "next week"]]), "e8")?;
    let corpus = [&e1, &e2, &e3, &e4, &e5, &e6, &e7, &e8];
    for (id, raw) in corpus {
        publish(&mut ws, id, raw).await?;
    }

    // ties order by event id ascending (event_hash blob compare)
    let mut tie = vec![e4.0.clone(), e5.0.clone(), e6.0.clone()];
    tie.sort();
    let expected_publication_desc = vec![
        e8.0.clone(),
        e7.0.clone(),
        e2.0.clone(),
        e3.0.clone(),
        tie[0].clone(),
        tie[1].clone(),
        tie[2].clone(),
        e1.0.clone(),
    ];

    // (a) ordered retrieval by publication time
    ws.send(r#"["REQ","order-all",{"kinds":[1],"limit":10,"order":"published_at"}]"#.into())
        .await?;
    let (events, notices) = collect_page(&mut ws, "order-all").await?;
    assert_eq!(events, expected_publication_desc);
    assert!(notices.is_empty(), "unexpected NOTICE: {notices:?}");

    // (b) paging via until/until_id across the tie group: page size 2
    // splits the three-way tie at 600; no skips, no repeats.
    let mut paged: Vec<String> = vec![];
    let mut cursor: Option<(u64, String)> = None;
    let publication = |id: &str| -> u64 {
        match id {
            i if i == e1.0 => 500,
            i if i == e2.0 => 900,
            i if i == e3.0 => 700,
            i if i == e7.0 => 1006,
            i if i == e8.0 => 1007,
            _ => 600,
        }
    };
    for page in 0..5 {
        let sub_id = format!("page-{page}");
        let filter = match &cursor {
            None => json!({"kinds": [1], "limit": 2, "order": "published_at"}),
            Some((until, until_id)) => json!({
                "kinds": [1], "limit": 2, "order": "published_at",
                "until": until, "until_id": until_id,
            }),
        };
        ws.send(json!(["REQ", sub_id, filter]).to_string().into())
            .await?;
        let (events, notices) = collect_page(&mut ws, &sub_id).await?;
        assert!(notices.is_empty(), "unexpected NOTICE: {notices:?}");
        if events.is_empty() {
            break;
        }
        let last = events.last().unwrap().clone();
        cursor = Some((publication(&last), last));
        paged.extend(events);
    }
    assert_eq!(paged, expected_publication_desc);

    // (c) a plain filter is untouched: created_at descending
    ws.send(r#"["REQ","stock",{"kinds":[1],"limit":10}]"#.into())
        .await?;
    let (events, notices) = collect_page(&mut ws, "stock").await?;
    let expected_created_at_desc = vec![
        e8.0.clone(),
        e7.0.clone(),
        e6.0.clone(),
        e5.0.clone(),
        e4.0.clone(),
        e3.0.clone(),
        e2.0.clone(),
        e1.0.clone(),
    ];
    assert_eq!(events, expected_created_at_desc);
    assert!(notices.is_empty(), "unexpected NOTICE: {notices:?}");

    // (d) strictness: unknown order value yields NOTICE and no events
    ws.send(r#"["REQ","bogus-order",{"kinds":[1],"order":"bogus"}]"#.into())
        .await?;
    let (events, notices) = collect_page(&mut ws, "bogus-order").await?;
    assert!(events.is_empty(), "bogus order returned events: {events:?}");
    assert!(
        notices.iter().any(|n| n.contains("order")),
        "expected a NOTICE naming order, got {notices:?}"
    );

    // (d) strictness: malformed until_id yields NOTICE and no events
    ws.send(
        r#"["REQ","bogus-cursor",{"kinds":[1],"order":"published_at","until":1000,"until_id":"nothex"}]"#
            .into(),
    )
    .await?;
    let (events, notices) = collect_page(&mut ws, "bogus-cursor").await?;
    assert!(events.is_empty(), "bogus until_id returned events: {events:?}");
    assert!(
        notices.iter().any(|n| n.contains("until_id")),
        "expected a NOTICE naming until_id, got {notices:?}"
    );

    ws.close(None).await?;
    let port = relay.port;
    relay
        .shutdown_tx
        .send(())
        .map_err(|e| anyhow!("shutdown send failed: {e}"))?;
    relay
        .handle
        .join()
        .map_err(|e| anyhow!("relay thread panicked: {e:?}"))?;
    assert!(common::port_is_available(port));
    Ok(())
}
