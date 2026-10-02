use crate::error::{Error, Result};
use crate::notice::Notice;
use crate::{event::Event, nip05::Nip05Name};
use std::time::Duration;
use nauthz_grpc::authorization_client::AuthorizationClient;
use nauthz_grpc::event::TagEntry;
use nauthz_grpc::{Decision, Event as GrpcEvent, EventReply, EventRequest};
use tracing::{info, warn};

pub mod nauthz_grpc {
    tonic::include_proto!("nauthz");
}

// A decision for the DB to act upon
pub trait AuthzDecision: Send + Sync {
    fn permitted(&self) -> bool;
    fn message(&self) -> Option<String>;
}

impl AuthzDecision for EventReply {
    fn permitted(&self) -> bool {
        self.decision == Decision::Permit as i32
    }
    fn message(&self) -> Option<String> {
        self.message.clone()
    }
}

// A connection to an event admission GRPC server
pub struct EventAuthzService {
    server_addr: String,
    conn: Option<AuthorizationClient<tonic::transport::Channel>>,
    // bound on one connect attempt and on one admission call
    timeout: Duration,
}

/// Text of the rejection sent when the admission service cannot decide.
pub const ADMISSION_UNAVAILABLE: &str = "event admission service unavailable";

/// Maps an admission result to a rejection notice, or to None when the event
/// is admitted. Only a permit admits: a deny or any error rejects, so the
/// relay never stores an event that the admission service did not permit.
#[must_use]
pub fn rejection(event_id: &str, decision: &Result<Box<dyn AuthzDecision>>) -> Option<Notice> {
    match decision {
        Ok(d) if d.permitted() => None,
        Ok(d) => Some(Notice::admission_denied(
            event_id.to_string(),
            &d.message().unwrap_or_default(),
        )),
        Err(_) => Some(Notice::error(event_id.to_string(), ADMISSION_UNAVAILABLE)),
    }
}

// conversion of Nip05Names into GRPC type
impl std::convert::From<Nip05Name> for nauthz_grpc::event_request::Nip05Name {
    fn from(value: Nip05Name) -> Self {
        nauthz_grpc::event_request::Nip05Name {
            local: value.local.clone(),
            domain: value.domain,
        }
    }
}

// conversion of event tags into gprc struct
fn tags_to_protobuf(tags: &[Vec<String>]) -> Vec<TagEntry> {
    tags.iter()
        .map(|x| TagEntry { values: x.clone() })
        .collect()
}

impl EventAuthzService {
    pub async fn connect(server_addr: &str, timeout: Duration) -> EventAuthzService {
        let mut eas = EventAuthzService {
            server_addr: server_addr.to_string(),
            conn: None,
            timeout,
        };
        eas.ready_connection().await;
        eas
    }

    pub async fn ready_connection(&mut self) {
        if self.conn.is_none() {
            let connect = AuthorizationClient::connect(self.server_addr.to_string());
            match tokio::time::timeout(self.timeout, connect).await {
                Ok(Ok(client)) => {
                    info!("connected to nostr authorization GRPC server");
                    self.conn = Some(client);
                }
                Ok(Err(msg)) => {
                    warn!("could not connect to nostr authz GRPC server: {:?}", msg);
                }
                Err(_) => {
                    warn!(
                        "could not connect to nostr authz GRPC server within {:?}",
                        self.timeout
                    );
                }
            }
        }
    }

    pub async fn admit_event(
        &mut self,
        event: &Event,
        ip: &str,
        origin: Option<String>,
        user_agent: Option<String>,
        nip05: Option<Nip05Name>,
        auth_pubkey: Option<Vec<u8>>,
    ) -> Result<Box<dyn AuthzDecision>> {
        self.ready_connection().await;
        let timeout = self.timeout;
        let call = self.call(event, ip, origin, user_agent, nip05, auth_pubkey);
        match tokio::time::timeout(timeout, call).await {
            Ok(res) => res,
            Err(_) => Err(Error::AuthzTimeout),
        }
    }

    async fn call(
        &mut self,
        event: &Event,
        ip: &str,
        origin: Option<String>,
        user_agent: Option<String>,
        nip05: Option<Nip05Name>,
        auth_pubkey: Option<Vec<u8>>,
    ) -> Result<Box<dyn AuthzDecision>> {
        let id_blob = hex::decode(&event.id)?;
        let pubkey_blob = hex::decode(&event.pubkey)?;
        let sig_blob = hex::decode(&event.sig)?;
        if let Some(ref mut c) = self.conn {
            let gevent = GrpcEvent {
                id: id_blob,
                pubkey: pubkey_blob,
                sig: sig_blob,
                created_at: event.created_at,
                kind: event.kind,
                content: event.content.clone(),
                tags: tags_to_protobuf(&event.tags),
            };
            let svr_res = c
                .event_admit(EventRequest {
                    event: Some(gevent),
                    ip_addr: Some(ip.to_string()),
                    origin,
                    user_agent,
                    auth_pubkey,
                    nip05: nip05.map(nauthz_grpc::event_request::Nip05Name::from),
                })
                .await?;
            let reply = svr_res.into_inner();
            Ok(Box::new(reply))
        } else {
            Err(Error::AuthzError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notice::EventResultStatus;

    const EVENT_ID: &str = "abc";

    fn reply(decision: Decision, message: Option<&str>) -> Result<Box<dyn AuthzDecision>> {
        Ok(Box::new(EventReply {
            decision: decision as i32,
            message: message.map(str::to_string),
        }))
    }

    fn rejected(decision: &Result<Box<dyn AuthzDecision>>) -> (String, EventResultStatus) {
        match rejection(EVENT_ID, decision) {
            Some(Notice::EventResult(res)) => {
                assert_eq!(res.id, EVENT_ID);
                (res.msg, res.status)
            }
            _ => panic!("expected a rejection"),
        }
    }

    #[test]
    fn permit_admits() {
        assert!(rejection(EVENT_ID, &reply(Decision::Permit, None)).is_none());
    }

    #[test]
    fn plain_deny_is_blocked() {
        let (msg, status) = rejected(&reply(Decision::Deny, Some("not on the list")));
        assert_eq!(msg, "blocked: not on the list");
        assert!(matches!(status, EventResultStatus::Blocked));
    }

    #[test]
    fn prefixed_deny_keeps_its_prefix() {
        let (msg, status) = rejected(&reply(Decision::Deny, Some("restricted: not authorized")));
        assert_eq!(msg, "restricted: not authorized");
        assert!(matches!(status, EventResultStatus::Restricted));
    }

    #[test]
    fn auth_required_deny_asks_for_auth() {
        let text = "auth-required: NIP-42 authentication required";
        let (msg, status) = rejected(&reply(Decision::Deny, Some(text)));
        assert_eq!(msg, text);
        assert!(matches!(status, EventResultStatus::AuthRequired));
    }

    #[test]
    fn deny_without_message_is_blocked() {
        let (msg, status) = rejected(&reply(Decision::Deny, None));
        assert_eq!(msg, "blocked: ");
        assert!(!status.to_bool());
    }

    #[test]
    fn error_rejects() {
        for err in [Error::AuthzError, Error::AuthzTimeout] {
            let (msg, status) = rejected(&Err(err));
            assert_eq!(msg, format!("error: {ADMISSION_UNAVAILABLE}"));
            assert!(matches!(status, EventResultStatus::Error));
        }
    }
}
