use crate::db::SubmittedEvent;
use crate::event::Event;
use crate::grpc_convert::{
    internal_event_to_proto, proto_event_to_internal, proto_filter_to_internal, relay_proto,
};
use crate::notice::Notice;
use crate::repo::NostrRepo;
use crate::server::NostrMetrics;
use crate::subscription::Subscription;
use relay_proto::relay_server::Relay;
use relay_proto::{
    AuthRequest, AuthResponse, EventEnvelope, PublishRequest, PublishResponse, QueryRequest,
    QueryResponse, SubscribeRequest, UnsubscribeRequest, UnsubscribeResponse,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

/// Per-subscription output channel capacity. Sized to absorb short bursts
/// without backpressuring the broadcast fan-out loop. Unlike broadcast_buffer
/// (which bounds the shared broadcast channel), this bounds a single client's
/// delivery queue.
const GRPC_STREAM_BUFFER: usize = 256;

type SubscribeStream = Pin<Box<dyn futures::Stream<Item = Result<EventEnvelope, Status>> + Send>>;

struct ConnectionState {
    auth_pubkey: Option<String>,
    subscriptions: HashMap<String, oneshot::Sender<()>>,
}

pub struct RelayService {
    event_tx: mpsc::Sender<SubmittedEvent>,
    bcast_tx: broadcast::Sender<Event>,
    repo: Arc<dyn NostrRepo>,
    settings: crate::config::Settings,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    metrics: NostrMetrics,
}

impl RelayService {
    pub fn new(
        event_tx: mpsc::Sender<SubmittedEvent>,
        bcast_tx: broadcast::Sender<Event>,
        repo: Arc<dyn NostrRepo>,
        settings: crate::config::Settings,
        metrics: NostrMetrics,
    ) -> Self {
        Self {
            event_tx,
            bcast_tx,
            repo,
            settings,
            connections: Arc::new(RwLock::new(HashMap::new())),
            metrics,
        }
    }

    // TODO: Uses remote_addr (IP:port) as session identity. Fragile behind
    // proxies or with connection pooling. Acceptable for current use case
    // where gRPC serves delos control plane with direct mesh connections.
    fn peer_key(req: &Request<impl std::any::Any>) -> String {
        req.remote_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[tonic::async_trait]
impl Relay for RelayService {
    type SubscribeStream = SubscribeStream;

    async fn publish(
        &self,
        request: Request<PublishRequest>,
    ) -> Result<Response<PublishResponse>, Status> {
        self.metrics.cmd_event.inc();
        let peer = Self::peer_key(&request);
        let req = request.into_inner();
        let proto_event = req
            .event
            .ok_or_else(|| Status::invalid_argument("missing event"))?;
        let event = proto_event_to_internal(&proto_event)
            .map_err(|e| Status::invalid_argument(format!("invalid event: {e}")))?;

        if let Err(e) = event.validate() {
            return Ok(Response::new(PublishResponse {
                accepted: false,
                message: format!("invalid event: {e}"),
            }));
        }

        if !event.is_valid_timestamp(self.settings.options.reject_future_seconds) {
            return Ok(Response::new(PublishResponse {
                accepted: false,
                message: "event timestamp out of range".to_string(),
            }));
        }

        let auth_pubkey = {
            let conns = self.connections.read().await;
            conns
                .get(&peer)
                .and_then(|cs| cs.auth_pubkey.as_ref())
                .and_then(|pk| hex::decode(pk).ok())
        };

        let (notice_tx, mut notice_rx) = mpsc::channel::<Notice>(1);
        let submitted = SubmittedEvent {
            event,
            notice_tx,
            source_ip: peer,
            origin: Some("grpc".to_string()),
            user_agent: Some("grpc-client".to_string()),
            auth_pubkey,
        };

        self.event_tx
            .send(submitted)
            .await
            .map_err(|_| Status::internal("event pipeline unavailable"))?;

        match notice_rx.recv().await {
            Some(Notice::EventResult(result)) => Ok(Response::new(PublishResponse {
                accepted: result.status.to_bool(),
                message: result.msg,
            })),
            Some(Notice::Message(msg)) => Ok(Response::new(PublishResponse {
                accepted: false,
                message: msg,
            })),
            _ => Ok(Response::new(PublishResponse {
                accepted: false,
                message: "no response from event pipeline".to_string(),
            })),
        }
    }

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        self.metrics.cmd_req.inc();
        let peer = Self::peer_key(&request);
        let req = request.into_inner();
        let sub_id = req.subscription_id.clone();

        if sub_id.is_empty() {
            return Err(Status::invalid_argument("subscription_id is required"));
        }
        if req.filters.is_empty() {
            return Err(Status::invalid_argument("at least one filter is required"));
        }

        let filters: Vec<_> = req.filters.iter().map(proto_filter_to_internal).collect();
        let subscription = Subscription {
            id: sub_id.clone(),
            filters,
        };

        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        {
            let mut conns = self.connections.write().await;
            let conn_state = conns.entry(peer.clone()).or_insert_with(|| ConnectionState {
                auth_pubkey: None,
                subscriptions: HashMap::new(),
            });
            if let Some(old_cancel) = conn_state.subscriptions.insert(sub_id.clone(), cancel_tx) {
                old_cancel.send(()).ok();
            }
        }

        let query_buffer = self.settings.limits.event_persist_buffer;
        let (stream_tx, stream_rx) =
            mpsc::channel::<Result<EventEnvelope, Status>>(GRPC_STREAM_BUFFER);
        let repo = self.repo.clone();
        let bcast_tx = self.bcast_tx.clone();
        let sub_for_live = subscription.clone();
        let connections = self.connections.clone();
        let metrics = self.metrics.clone();

        tokio::spawn(async move {
            // Subscribe to broadcast BEFORE historical query to prevent event gap.
            // Events broadcast during the query phase are buffered and replayed
            // after EOSE, mirroring the WebSocket handler pattern (server.rs:1128).
            let mut bcast_rx = bcast_tx.subscribe();
            let mut buffered_events: Vec<Event> = Vec::new();

            'subscribe: {
                let (query_tx, mut query_rx) =
                    mpsc::channel::<crate::db::QueryResult>(query_buffer);
                let (abandon_tx, abandon_rx) = oneshot::channel::<()>();

                if subscription.needs_historical_events() {
                    let query_sub = subscription.clone();
                    let query_repo = repo.clone();
                    let client_id = format!("grpc-{}", peer);
                    tokio::spawn(async move {
                        if let Err(e) = query_repo
                            .query_subscription(query_sub, client_id, query_tx, abandon_rx)
                            .await
                        {
                            warn!("grpc query_subscription error: {}", e);
                        }
                    });

                    // Drain historical results while buffering live broadcast events.
                    // Biased toward query results to minimize query duration.
                    loop {
                        tokio::select! {
                            biased;
                            qr = query_rx.recv() => {
                                match qr {
                                    Some(qr) => {
                                        let event: Result<Event, _> =
                                            serde_json::from_str(&qr.event);
                                        match event {
                                            Ok(e) => match internal_event_to_proto(&e) {
                                                Ok(pe) => {
                                                    let envelope = EventEnvelope {
                                                        subscription_id: sub_id.clone(),
                                                        payload: Some(
                                                            relay_proto::event_envelope::Payload::Event(pe),
                                                        ),
                                                    };
                                                    if stream_tx.send(Ok(envelope)).await.is_err() {
                                                        abandon_tx.send(()).ok();
                                                        break 'subscribe;
                                                    }
                                                    metrics.sent_events.with_label_values(&["grpc-db"]).inc();
                                                }
                                                Err(e) => {
                                                    debug!("grpc: failed to convert stored event: {}", e);
                                                }
                                            },
                                            Err(e) => {
                                                debug!("grpc: failed to parse stored event JSON: {}", e);
                                            }
                                        }
                                    }
                                    None => break,
                                }
                            }
                            result = bcast_rx.recv() => {
                                match result {
                                    Ok(event) => {
                                        if sub_for_live.interested_in_event(&event) {
                                            buffered_events.push(event);
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        warn!("grpc subscriber lagged during query, missed {} events", n);
                                    }
                                    Err(broadcast::error::RecvError::Closed) => {
                                        break 'subscribe;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    drop(query_tx);
                    drop(abandon_tx);
                }

                let eose = EventEnvelope {
                    subscription_id: sub_id.clone(),
                    payload: Some(relay_proto::event_envelope::Payload::Eose(true)),
                };
                if stream_tx.send(Ok(eose)).await.is_err() {
                    break 'subscribe;
                }

                // Replay events received during the historical query phase
                for event in buffered_events {
                    match internal_event_to_proto(&event) {
                        Ok(pe) => {
                            let envelope = EventEnvelope {
                                subscription_id: sub_id.clone(),
                                payload: Some(
                                    relay_proto::event_envelope::Payload::Event(pe),
                                ),
                            };
                            if stream_tx.send(Ok(envelope)).await.is_err() {
                                break 'subscribe;
                            }
                            metrics
                                .sent_events
                                .with_label_values(&["grpc-realtime"])
                                .inc();
                        }
                        Err(e) => {
                            debug!("grpc: failed to convert buffered event: {}", e);
                        }
                    }
                }

                let mut cancel_rx = cancel_rx;
                loop {
                    tokio::select! {
                        result = bcast_rx.recv() => {
                            match result {
                                Ok(event) => {
                                    if sub_for_live.interested_in_event(&event) {
                                        match internal_event_to_proto(&event) {
                                            Ok(pe) => {
                                                let envelope = EventEnvelope {
                                                    subscription_id: sub_id.clone(),
                                                    payload: Some(relay_proto::event_envelope::Payload::Event(pe)),
                                                };
                                                if stream_tx.send(Ok(envelope)).await.is_err() {
                                                    break;
                                                }
                                                metrics.sent_events.with_label_values(&["grpc-realtime"]).inc();
                                            }
                                            Err(e) => {
                                                debug!("grpc: failed to convert broadcast event: {}", e);
                                            }
                                        }
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    warn!("grpc subscriber lagged, missed {} events", n);
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    break;
                                }
                            }
                        }
                        _ = &mut cancel_rx => {
                            debug!("grpc subscription cancelled: {}", sub_id);
                            break;
                        }
                    }
                }
            }

            // Cleanup: remove subscription entry to prevent memory leak.
            // Runs on all exit paths (stream close, cancel, broadcast close).
            let mut conns = connections.write().await;
            if let Some(conn_state) = conns.get_mut(&peer) {
                conn_state.subscriptions.remove(&sub_id);
                if conn_state.subscriptions.is_empty() && conn_state.auth_pubkey.is_none() {
                    conns.remove(&peer);
                }
            }
        });

        let stream = ReceiverStream::new(stream_rx);
        Ok(Response::new(Box::pin(stream) as Self::SubscribeStream))
    }

    async fn unsubscribe(
        &self,
        request: Request<UnsubscribeRequest>,
    ) -> Result<Response<UnsubscribeResponse>, Status> {
        self.metrics.cmd_close.inc();
        let peer = Self::peer_key(&request);
        let req = request.into_inner();

        let mut conns = self.connections.write().await;
        if let Some(conn_state) = conns.get_mut(&peer) {
            if let Some(cancel_tx) = conn_state.subscriptions.remove(&req.subscription_id) {
                cancel_tx.send(()).ok();
                info!(
                    "grpc: unsubscribed {} for peer {}",
                    req.subscription_id, peer
                );
            }
        }

        Ok(Response::new(UnsubscribeResponse {}))
    }

    // Auth accepts a kind 22242 event with a valid signature. Challenge-response
    // (challenge tag, relay tag verification) is intentionally omitted: the gRPC
    // transport serves the delos control plane where all connections are mesh-only
    // between trusted processes. There is no untrusted network exposure. If gRPC
    // is ever exposed to untrusted clients, NIP-42 challenge-response must be
    // implemented here.
    async fn auth(
        &self,
        request: Request<AuthRequest>,
    ) -> Result<Response<AuthResponse>, Status> {
        self.metrics.cmd_auth.inc();
        let peer = Self::peer_key(&request);
        let req = request.into_inner();
        let proto_event = req
            .auth_event
            .ok_or_else(|| Status::invalid_argument("missing auth_event"))?;
        let event = proto_event_to_internal(&proto_event)
            .map_err(|e| Status::invalid_argument(format!("invalid auth event: {e}")))?;

        if let Err(e) = event.validate() {
            return Ok(Response::new(AuthResponse {
                authenticated: false,
                message: format!("invalid event: {e}"),
            }));
        }

        if event.kind != 22242 {
            return Ok(Response::new(AuthResponse {
                authenticated: false,
                message: "auth event must be kind 22242".to_string(),
            }));
        }

        let pubkey = event.pubkey.clone();
        {
            let mut conns = self.connections.write().await;
            let conn_state = conns.entry(peer.clone()).or_insert_with(|| ConnectionState {
                auth_pubkey: None,
                subscriptions: HashMap::new(),
            });
            conn_state.auth_pubkey = Some(pubkey.clone());
        }

        let short_pk: String = pubkey.chars().take(8).collect();
        info!("grpc: peer {} authenticated as {}", peer, short_pk);

        Ok(Response::new(AuthResponse {
            authenticated: true,
            message: String::new(),
        }))
    }

    async fn query(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        let req = request.into_inner();
        if req.filters.is_empty() {
            return Err(Status::invalid_argument("at least one filter is required"));
        }

        let filters: Vec<_> = req.filters.iter().map(proto_filter_to_internal).collect();
        let subscription = Subscription {
            id: "grpc-query".to_string(),
            filters,
        };

        let query_buffer = self.settings.limits.event_persist_buffer;
        let (query_tx, mut query_rx) = mpsc::channel::<crate::db::QueryResult>(query_buffer);
        let (abandon_tx, abandon_rx) = oneshot::channel::<()>();

        let repo = self.repo.clone();
        tokio::spawn(async move {
            if let Err(e) = repo
                .query_subscription(
                    subscription,
                    "grpc-query".to_string(),
                    query_tx,
                    abandon_rx,
                )
                .await
            {
                warn!("grpc query error: {}", e);
            }
        });

        let mut events = Vec::new();
        while let Some(qr) = query_rx.recv().await {
            let event: Result<Event, _> = serde_json::from_str(&qr.event);
            match event {
                Ok(e) => match internal_event_to_proto(&e) {
                    Ok(pe) => events.push(pe),
                    Err(e) => {
                        debug!("grpc: failed to convert query event: {}", e);
                    }
                },
                Err(e) => {
                    debug!("grpc: failed to parse query event JSON: {}", e);
                }
            }
        }

        drop(abandon_tx);

        Ok(Response::new(QueryResponse { events }))
    }
}
