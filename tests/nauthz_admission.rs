//! Event admission through an in-process authorization server: the relay
//! client must fail closed, within its deadline, when the server cannot decide.
use anyhow::Result;
use nostr_rs_relay::error::Error;
use nostr_rs_relay::event::Event;
use nostr_rs_relay::nauthz::nauthz_grpc::authorization_server::{
    Authorization, AuthorizationServer,
};
use nostr_rs_relay::nauthz::nauthz_grpc::{Decision, EventReply, EventRequest};
use nostr_rs_relay::nauthz::{rejection, AuthzDecision, EventAuthzService};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status};

/// Admission deadline handed to the client under test.
const ADMISSION_DEADLINE: Duration = Duration::from_millis(200);
/// Upper bound on any one test, so a defect fails rather than hangs.
const TEST_GUARD: Duration = Duration::from_secs(10);

/// What the in-process server does with each admission call.
#[derive(Clone, Copy)]
enum Behavior {
    Permit,
    NeverAnswer,
}

struct Server {
    behavior: Behavior,
}

#[tonic::async_trait]
impl Authorization for Server {
    async fn event_admit(
        &self,
        _request: Request<EventRequest>,
    ) -> std::result::Result<Response<EventReply>, Status> {
        match self.behavior {
            Behavior::Permit => Ok(Response::new(EventReply {
                decision: Decision::Permit as i32,
                message: None,
            })),
            Behavior::NeverAnswer => std::future::pending().await,
        }
    }
}

/// Starts an authorization server on a free loopback port and returns its URL.
async fn start_server(behavior: Behavior) -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let incoming = Box::pin(futures::stream::unfold(listener, |l| async move {
        let conn = l.accept().await.map(|(stream, _)| stream);
        Some((conn, l))
    }));
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(AuthorizationServer::new(Server { behavior }))
            .serve_with_incoming(incoming),
    );
    Ok(format!("http://{addr}"))
}

/// A loopback address with no listener behind it.
async fn closed_address() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    drop(listener);
    Ok(format!("http://{addr}"))
}

fn event() -> Event {
    Event {
        id: "00".repeat(32),
        pubkey: "11".repeat(32),
        delegated_by: None,
        created_at: 0,
        kind: 1,
        tags: vec![],
        content: String::new(),
        sig: "22".repeat(64),
        tagidx: None,
    }
}

async fn admit(addr: &str) -> nostr_rs_relay::error::Result<Box<dyn AuthzDecision>> {
    let mut client = EventAuthzService::connect(addr, ADMISSION_DEADLINE).await;
    client
        .admit_event(&event(), "127.0.0.1", None, None, None, None)
        .await
}

#[tokio::test]
async fn unanswered_call_is_rejected_at_the_deadline() -> Result<()> {
    let addr = start_server(Behavior::NeverAnswer).await?;
    let started = Instant::now();
    let decision = tokio::time::timeout(TEST_GUARD, admit(&addr)).await?;
    let elapsed = started.elapsed();
    assert!(matches!(decision, Err(Error::AuthzTimeout)));
    assert!(elapsed >= ADMISSION_DEADLINE, "returned before the deadline: {elapsed:?}");
    assert!(rejection("id", &decision).is_some());
    Ok(())
}

#[tokio::test]
async fn unreachable_server_is_rejected() -> Result<()> {
    let addr = closed_address().await?;
    let decision = tokio::time::timeout(TEST_GUARD, admit(&addr)).await?;
    assert!(decision.is_err());
    assert!(rejection("id", &decision).is_some());
    Ok(())
}

#[tokio::test]
async fn permit_is_admitted() -> Result<()> {
    let addr = start_server(Behavior::Permit).await?;
    let decision = tokio::time::timeout(TEST_GUARD, admit(&addr)).await?;
    assert!(matches!(&decision, Ok(d) if d.permitted()));
    assert!(rejection("id", &decision).is_none());
    Ok(())
}
