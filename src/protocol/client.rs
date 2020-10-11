use failure::Error;
use futures::future::BoxFuture;
use futures::{Future, FutureExt, TryFutureExt};
use hyper::client::connect::Connect;
use hyper::{Body, Request, Response};
use mockall::automock;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rand::Rng;
use serde_json::json;

use std::fmt::{Debug, Display};
use std::{pin::Pin, sync::Arc};

use crate::json::signed::Signed;
use crate::protocol::server_resolver::MatrixConnector;
use crate::protocol::{Event, RoomVersion};
use tracing::{field, instrument, Span};
use tracing_futures::Instrument;

pub fn enc(s: &str) -> impl Display + '_ {
    utf8_percent_encode(s, NON_ALPHANUMERIC)
}

#[automock]
pub trait Requester: Send + Sync {
    fn request(
        &self,
        request: Request<hyper::Body>,
    ) -> Pin<Box<dyn Future<Output = Result<Response<Body>, Error>> + Send>>;
}

impl<C> Requester for hyper::Client<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    fn request(
        &self,
        request: Request<hyper::Body>,
    ) -> Pin<Box<dyn Future<Output = Result<Response<Body>, Error>> + Send>>
    {
        let method = request.method().clone();
        let uri = request.uri().to_string();

        hyper::Client::request(self, request)
            .err_into()
            .inspect_ok(|resp: &Response<hyper::Body>| {
                Span::current().record("status", &resp.status().as_u16());
            })
            .inspect_err(|err: &Error| {
                Span::current().record("error", &&err.to_string()[..]);
            })
            .instrument(tracing::info_span!(
                "outbound_request",
                method = method.as_str(),
                uri = &uri as &str,
                status = field::Empty,
                error = field::Empty,
                span.kind = "client",
            ))
            .boxed()
    }
}

impl Requester for () {
    fn request(
        &self,
        _request: Request<hyper::Body>,
    ) -> Pin<Box<dyn Future<Output = Result<Response<Body>, Error>> + Send>>
    {
        futures::future::err(format_err!("HTTP client disabled")).boxed()
    }
}

#[derive(Clone)]
pub struct HyperFederationClient {
    client: Arc<dyn Requester>,
    server_name: String,
    key_name: String,
    secret_key: sodiumoxide::crypto::sign::SecretKey,
}

impl HyperFederationClient {
    pub fn new<C: Requester + 'static>(
        client: C,
        server_name: String,
        key_name: String,
        secret_key: sodiumoxide::crypto::sign::SecretKey,
    ) -> HyperFederationClient {
        HyperFederationClient {
            client: Arc::new(client),
            server_name,
            key_name,
            secret_key,
        }
    }

    pub fn with_arc_requester<C: Requester + 'static>(
        client: Arc<C>,
        server_name: String,
        key_name: String,
        secret_key: sodiumoxide::crypto::sign::SecretKey,
    ) -> HyperFederationClient {
        HyperFederationClient {
            client,
            server_name,
            key_name,
            secret_key,
        }
    }

    #[instrument(err, skip(self))]
    pub async fn get_event<R: RoomVersion>(
        &self,
        destination: &str,
        _room_id: &str,
        event_id: &str,
    ) -> Result<R::Event, Error> {
        let path = format!("/_matrix/federation/v1/event/{}", enc(event_id),);

        let auth_header =
            self.make_auth_header::<()>("GET", &path, destination, None);

        let uri = hyper::Uri::builder()
            .scheme("matrix")
            .authority(&destination as &str)
            .path_and_query(&path as &str)
            .build()?;

        let request = hyper::Request::get(uri)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(hyper::Body::empty())?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| format_err!("{}", e))?;

        if !response.status().is_success() {
            bail!("Got {} response code for /event", response.status());
        }

        let resp_bytes = hyper::body::to_bytes(response.into_body()).await?;

        let resp: Transaction<R> = serde_json::from_slice(&resp_bytes)?;

        if resp.pdus.len() != 1 {
            bail!("Got {} PDUs in /event response", resp.pdus.len());
        }

        let event = resp
            .pdus
            .into_iter()
            .next()
            .expect("response has at least one item");

        if event.event_id() != event_id {
            bail!("Got wrong event from /event")
        }

        Ok(event)
    }

    #[instrument(err, skip(self))]
    pub async fn make_join(
        &self,
        destination: &str,
        room_id: &str,
        user_id: &str,
        valid_room_versions: &[&'static str],
    ) -> Result<MakeJoinResponse, Error> {
        let mut query = String::with_capacity(valid_room_versions.len() * 6);
        for v in valid_room_versions {
            query.push_str("ver=");
            query.push_str(*v);
            query.push('&');
        }

        query.pop(); // Remove the last '&'

        let path = format!(
            "/_matrix/federation/v1/make_join/{}/{}?{}",
            enc(room_id),
            enc(user_id),
            query,
        );

        let auth_header =
            self.make_auth_header::<()>("GET", &path, destination, None);

        let uri = hyper::Uri::builder()
            .scheme("matrix")
            .authority(&destination as &str)
            .path_and_query(&path as &str)
            .build()?;

        let request = hyper::Request::get(uri)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(hyper::Body::empty())?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| format_err!("{}", e))?;

        if !response.status().is_success() {
            bail!("Got {} response code for /make_join", response.status());
        }

        let resp_bytes = hyper::body::to_bytes(response.into_body()).await?;

        let resp: MakeJoinResponse = serde_json::from_slice(&resp_bytes)?;

        Ok(resp)
    }

    #[instrument(err, skip(self))]
    pub async fn send_join<R: RoomVersion>(
        &self,
        destination: &str,
        room_id: &str,
        event: &R::Event,
    ) -> Result<SendJoinResponse<R>, Error> {
        let path = format!(
            "/_matrix/federation/v2/send_join/{}/{}",
            enc(room_id),
            enc(event.event_id()),
        );

        let auth_header =
            self.make_auth_header("PUT", &path, destination, Some(&event));

        let uri = hyper::Uri::builder()
            .scheme("matrix")
            .authority(&destination as &str)
            .path_and_query(&path as &str)
            .build()?;

        let request = hyper::Request::put(uri)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&event)?.into())?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| format_err!("{}", e))?;

        if !response.status().is_success() {
            bail!("Got {} response code for /send_join", response.status());
        }

        let resp_bytes = hyper::body::to_bytes(response.into_body()).await?;

        let resp = serde_json::from_slice(&resp_bytes)?;

        Ok(resp)
    }

    /// Send an invite to the remote server to be signed. Callers should check
    /// that the returned event passes signature checks and otherwise matches
    /// the event given.
    #[instrument(err, skip(self))]
    pub async fn invite<R: RoomVersion>(
        &self,
        destination: &str,
        room_id: &str,
        event: &R::Event,
        invite_room_state: &[&R::Event],
    ) -> Result<InviteResponse<R>, Error> {
        let path = format!(
            "/_matrix/federation/v2/invite/{}/{}",
            enc(room_id),
            enc(event.event_id()),
        );

        // TODO: Add invite room state.
        let payload = json!({ "event": &event, "room_version": R::VERSION, "invite_room_state": [] });

        let auth_header =
            self.make_auth_header("PUT", &path, destination, Some(&payload));

        let uri = hyper::Uri::builder()
            .scheme("matrix")
            .authority(&destination as &str)
            .path_and_query(&path as &str)
            .build()?;

        let request = hyper::Request::put(uri)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&payload)?.into())?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| format_err!("{}", e))?;

        if !response.status().is_success() {
            bail!("Got {} response code for /invite", response.status());
        }

        let resp_bytes = hyper::body::to_bytes(response.into_body()).await?;

        let resp = serde_json::from_slice(&resp_bytes)?;

        Ok(resp)
    }

    #[instrument(err, skip(self))]
    pub async fn get_missing_events<R: RoomVersion>(
        &self,
        destination: &str,
        room_id: &str,
        earliest_events: &[&str],
        latest_events: &[&str],
    ) -> Result<Vec<R::Event>, Error> {
        let path = format!(
            "/_matrix/federation/v1/get_missing_events/{}",
            enc(room_id),
        );

        // TODO: Fill out min_depth
        let content = &json!({
            "limit": 10,
            "min_depth": 0,
            "earliest_evnets": earliest_events,
            "latest_events": latest_events,
        });

        let auth_header =
            self.make_auth_header("POST", &path, destination, Some(&content));

        let uri = hyper::Uri::builder()
            .scheme("matrix")
            .authority(&destination as &str)
            .path_and_query(&path as &str)
            .build()?;

        let request = hyper::Request::post(uri)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&content)?.into())?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| format_err!("{}", e))?;

        if !response.status().is_success() {
            bail!(
                "Got {} response code for /get_missing_events",
                response.status()
            );
        }

        let resp_bytes = hyper::body::to_bytes(response.into_body()).await?;

        let resp: GetMissingEventsResponse<R::Event> =
            serde_json::from_slice(&resp_bytes)?;

        Ok(resp.events)
    }

    #[instrument(err, skip(self))]
    pub async fn get_state_ids(
        &self,
        destination: &str,
        room_id: &str,
        event_id: &str,
    ) -> Result<GetStateIdsResponse, Error> {
        let path = format!(
            "/_matrix/federation/v1/state_ids/{}?event_id={}",
            enc(room_id),
            enc(event_id)
        );

        let auth_header =
            self.make_auth_header::<()>("GET", &path, destination, None);

        let uri = hyper::Uri::builder()
            .scheme("matrix")
            .authority(&destination as &str)
            .path_and_query(&path as &str)
            .build()?;

        let request = hyper::Request::get(uri)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .body(hyper::Body::empty())?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| format_err!("{}", e))?;

        if !response.status().is_success() {
            bail!(
                "Got {} response code for /get_missing_events",
                response.status()
            );
        }

        let resp_bytes = hyper::body::to_bytes(response.into_body()).await?;

        let resp: GetStateIdsResponse = serde_json::from_slice(&resp_bytes)?;

        Ok(resp)
    }

    fn make_auth_header<T: serde::Serialize>(
        &self,
        method: &'static str,
        path: &str,
        destination: &str,
        content: Option<T>,
    ) -> String {
        let request_json = RequestJson {
            method,
            uri: path,
            origin: &self.server_name,
            destination,
            content,
        };

        let signed: Signed<_> = Signed::wrap(request_json).unwrap();
        let sig = signed.sign_detached(&self.secret_key);
        let b64_sig = base64::encode_config(&sig, base64::STANDARD_NO_PAD);

        format!(
            r#"X-Matrix origin={},key="{}",sig="{}""#,
            &self.server_name, &self.key_name, b64_sig,
        )
    }
}

impl Debug for HyperFederationClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyperFederationClient")
            .field("server_name", &self.server_name)
            .field("key_name", &self.key_name)
            .field("client", &"...")
            .field("secret_key", &"...")
            .finish()
    }
}

pub trait TransactionSender {
    /// Queues up an event to be sent to the given destination. Future resolves
    /// when succesfully *queued*, e.g. its been saved to a persistent queue.
    fn send_event<R: RoomVersion>(
        &self,
        destination: String,
        event: R::Event,
    ) -> BoxFuture<Result<(), Error>>;
}

#[derive(Clone)]
pub struct MemoryTransactionSender {
    pub client: hyper::Client<MatrixConnector>,
    pub server_name: String,
    pub key_name: String,
    pub secret_key: sodiumoxide::crypto::sign::SecretKey,
}

impl TransactionSender for MemoryTransactionSender {
    fn send_event<R: RoomVersion>(
        &self,
        destination: String,
        event: R::Event,
    ) -> BoxFuture<Result<(), Error>> {
        let mut rng = rand::thread_rng();
        let txn_id: String = std::iter::repeat(())
            .map(|()| rng.sample(rand::distributions::Alphanumeric))
            .take(7)
            .collect();

        let path = format!("/_matrix/federation/v1/send/{}", txn_id);

        let client = self.client.clone();

        let content = serde_json::json!({
            "pdus": [event],
            "origin": &self.server_name,
            "origin_server_ts": 0,
        });

        let request_json = RequestJson {
            method: "PUT",
            uri: &path,
            origin: &self.server_name,
            destination: &destination,
            content: Some(&content),
        };

        let signed: Signed<_> = Signed::wrap(request_json).unwrap();
        let sig = signed.sign_detached(&self.secret_key);
        let b64_sig = base64::encode_config(&sig, base64::STANDARD_NO_PAD);

        let auth_header = format!(
            r#"X-Matrix origin={},key="{}",sig="{}""#,
            &self.server_name, &self.key_name, b64_sig,
        );

        async move {
            let uri = hyper::Uri::builder()
                .scheme("matrix")
                .authority(&destination as &str)
                .path_and_query(&path as &str)
                .build()?;

            let request = hyper::Request::put(uri)
                .header("Authorization", auth_header)
                .header("Content-Type", "application/json")
                .body(serde_json::to_vec(&content)?.into())?;

            client
                .request(request)
                .await
                .map_err(|e| format_err!("{}", e))?;

            Ok(())
        }
        .boxed()
    }
}

#[derive(Serialize)]
struct RequestJson<'a, T> {
    method: &'a str,
    uri: &'a str,
    origin: &'a str,
    destination: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<T>,
}

#[derive(Serialize, Deserialize)]
pub struct GetMissingEventsResponse<E> {
    pub events: Vec<E>,
}

#[derive(Serialize, Deserialize)]
pub struct GetStateIdsResponse {
    pub auth_chain_ids: Vec<String>,
    pub pdu_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Transaction<R: RoomVersion> {
    pub pdus: Vec<R::Event>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MakeJoinResponse {
    pub room_version: String,
    pub event: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SendJoinResponse<R: RoomVersion> {
    pub auth_chain: Vec<R::Event>,
    pub state: Vec<R::Event>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InviteResponse<R: RoomVersion> {
    pub event: R::Event,
}
