use crate::event::Event;
use crate::subscription::{ReqFilter, TagOperand};
use std::collections::{HashMap, HashSet};

pub mod relay_proto {
    tonic::include_proto!("nostr.relay.v1");
}

pub fn proto_event_to_internal(pe: &relay_proto::Event) -> Result<Event, String> {
    let id = hex::encode(&pe.id);
    if pe.id.len() != 32 {
        return Err(format!("invalid event id length: {}", pe.id.len()));
    }
    let pubkey = hex::encode(&pe.pubkey);
    if pe.pubkey.len() != 32 {
        return Err(format!("invalid pubkey length: {}", pe.pubkey.len()));
    }
    let sig = hex::encode(&pe.sig);
    if pe.sig.len() != 64 {
        return Err(format!("invalid sig length: {}", pe.sig.len()));
    }
    let created_at = pe.created_at as u64;
    let kind = pe.kind as u64;
    let tags: Vec<Vec<String>> = pe.tags.iter().map(|t| t.values.clone()).collect();
    let content = pe.content.clone();
    Ok(Event {
        id,
        pubkey,
        delegated_by: None,
        created_at,
        kind,
        tags,
        content,
        sig,
        tagidx: None,
    })
}

pub fn internal_event_to_proto(e: &Event) -> Result<relay_proto::Event, String> {
    let id = hex::decode(&e.id).map_err(|err| format!("invalid event id hex: {err}"))?;
    let pubkey = hex::decode(&e.pubkey).map_err(|err| format!("invalid pubkey hex: {err}"))?;
    let sig = hex::decode(&e.sig).map_err(|err| format!("invalid sig hex: {err}"))?;
    let tags: Vec<relay_proto::Tag> = e
        .tags
        .iter()
        .map(|t| relay_proto::Tag {
            values: t.clone(),
        })
        .collect();
    Ok(relay_proto::Event {
        id,
        pubkey,
        created_at: e.created_at as i64,
        kind: e.kind as i32,
        tags,
        content: e.content.clone(),
        sig,
    })
}

pub fn proto_filter_to_internal(pf: &relay_proto::Filter) -> ReqFilter {
    let ids: Option<Vec<String>> = if pf.ids.is_empty() {
        None
    } else {
        Some(pf.ids.iter().map(hex::encode).collect())
    };
    let authors: Option<Vec<String>> = if pf.authors.is_empty() {
        None
    } else {
        Some(pf.authors.iter().map(hex::encode).collect())
    };
    let kinds: Option<Vec<u64>> = if pf.kinds.is_empty() {
        None
    } else {
        Some(pf.kinds.iter().map(|k| *k as u64).collect())
    };
    let since: Option<u64> = if pf.since == 0 {
        None
    } else {
        Some(pf.since as u64)
    };
    let until: Option<u64> = if pf.until == 0 {
        None
    } else {
        Some(pf.until as u64)
    };
    let limit: Option<u64> = if pf.limit == 0 {
        None
    } else {
        Some(pf.limit as u64)
    };
    let tags: Option<HashMap<char, TagOperand>> = if pf.tags.is_empty() {
        None
    } else {
        let mut tag_map = HashMap::new();
        for (key, string_list) in &pf.tags {
            if let Some(tag_char) = key.strip_prefix('#').and_then(|s| s.chars().next()) {
                let set: HashSet<String> = string_list.values.iter().cloned().collect();
                tag_map.insert(tag_char, TagOperand::Or(set));
            }
        }
        if tag_map.is_empty() {
            None
        } else {
            Some(tag_map)
        }
    };
    ReqFilter {
        ids,
        kinds,
        since,
        until,
        authors,
        limit,
        tags,
        force_no_match: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_proto_event() -> relay_proto::Event {
        relay_proto::Event {
            id: vec![0xab; 32],
            pubkey: vec![0xcd; 32],
            created_at: 1700000000,
            kind: 1,
            tags: vec![
                relay_proto::Tag {
                    values: vec!["p".to_string(), "deadbeef".to_string()],
                },
                relay_proto::Tag {
                    values: vec!["e".to_string(), "cafebabe".to_string()],
                },
            ],
            content: "hello world".to_string(),
            sig: vec![0xef; 64],
        }
    }

    fn make_test_internal_event() -> Event {
        Event {
            id: hex::encode(vec![0xab; 32]),
            pubkey: hex::encode(vec![0xcd; 32]),
            delegated_by: None,
            created_at: 1700000000,
            kind: 1,
            tags: vec![
                vec!["p".to_string(), "deadbeef".to_string()],
                vec!["e".to_string(), "cafebabe".to_string()],
            ],
            content: "hello world".to_string(),
            sig: hex::encode(vec![0xef; 64]),
            tagidx: None,
        }
    }

    #[test]
    fn test_proto_to_internal_roundtrip() {
        let proto_event = make_test_proto_event();
        let internal = proto_event_to_internal(&proto_event).unwrap();
        let back = internal_event_to_proto(&internal).unwrap();
        assert_eq!(proto_event.id, back.id);
        assert_eq!(proto_event.pubkey, back.pubkey);
        assert_eq!(proto_event.sig, back.sig);
        assert_eq!(proto_event.created_at, back.created_at);
        assert_eq!(proto_event.kind, back.kind);
        assert_eq!(proto_event.content, back.content);
        assert_eq!(proto_event.tags.len(), back.tags.len());
        for (a, b) in proto_event.tags.iter().zip(back.tags.iter()) {
            assert_eq!(a.values, b.values);
        }
    }

    #[test]
    fn test_internal_to_proto_roundtrip() {
        let internal = make_test_internal_event();
        let proto = internal_event_to_proto(&internal).unwrap();
        let back = proto_event_to_internal(&proto).unwrap();
        assert_eq!(internal.id, back.id);
        assert_eq!(internal.pubkey, back.pubkey);
        assert_eq!(internal.sig, back.sig);
        assert_eq!(internal.created_at, back.created_at);
        assert_eq!(internal.kind, back.kind);
        assert_eq!(internal.content, back.content);
        assert_eq!(internal.tags, back.tags);
    }

    #[test]
    fn test_proto_event_invalid_id_length() {
        let mut pe = make_test_proto_event();
        pe.id = vec![0xab; 16];
        let result = proto_event_to_internal(&pe);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid event id length"));
    }

    #[test]
    fn test_proto_event_invalid_pubkey_length() {
        let mut pe = make_test_proto_event();
        pe.pubkey = vec![0xcd; 16];
        let result = proto_event_to_internal(&pe);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid pubkey length"));
    }

    #[test]
    fn test_proto_event_invalid_sig_length() {
        let mut pe = make_test_proto_event();
        pe.sig = vec![0xef; 32];
        let result = proto_event_to_internal(&pe);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid sig length"));
    }

    #[test]
    fn test_proto_filter_to_internal() {
        let pf = relay_proto::Filter {
            ids: vec![vec![0xaa; 32]],
            authors: vec![vec![0xbb; 32]],
            kinds: vec![1, 30023],
            tags: {
                let mut m = HashMap::new();
                m.insert(
                    "#p".to_string(),
                    relay_proto::StringList {
                        values: vec!["deadbeef".to_string()],
                    },
                );
                m
            },
            since: 1700000000,
            until: 1700099999,
            limit: 100,
        };
        let rf = proto_filter_to_internal(&pf);
        assert_eq!(rf.ids.as_ref().unwrap().len(), 1);
        assert_eq!(rf.ids.as_ref().unwrap()[0], hex::encode(vec![0xaa; 32]));
        assert_eq!(rf.authors.as_ref().unwrap().len(), 1);
        assert_eq!(rf.kinds.as_ref().unwrap(), &vec![1u64, 30023u64]);
        assert_eq!(rf.since, Some(1700000000));
        assert_eq!(rf.until, Some(1700099999));
        assert_eq!(rf.limit, Some(100));
        let tags = rf.tags.as_ref().unwrap();
        assert!(tags.contains_key(&'p'));
    }

    #[test]
    fn test_proto_filter_empty_fields() {
        let pf = relay_proto::Filter::default();
        let rf = proto_filter_to_internal(&pf);
        assert!(rf.ids.is_none());
        assert!(rf.authors.is_none());
        assert!(rf.kinds.is_none());
        assert!(rf.since.is_none());
        assert!(rf.until.is_none());
        assert!(rf.limit.is_none());
        assert!(rf.tags.is_none());
        assert!(!rf.force_no_match);
    }

    #[test]
    fn test_empty_tags_event() {
        let mut pe = make_test_proto_event();
        pe.tags.clear();
        let internal = proto_event_to_internal(&pe).unwrap();
        assert!(internal.tags.is_empty());
        let back = internal_event_to_proto(&internal).unwrap();
        assert!(back.tags.is_empty());
    }
}
