use failure::Error;
use futures::future::BoxFuture;
use serde::Serialize;

use crate::protocol::RoomVersion;

pub mod basic;

#[derive(Debug)]
pub enum FederationAPIError {
    HttpResponse(http::Response<Vec<u8>>),
    Error(Error),
}

impl From<Error> for FederationAPIError {
    fn from(err: Error) -> FederationAPIError {
        FederationAPIError::Error(err)
    }
}

pub type FederationResult<T> = Result<T, FederationAPIError>;

pub trait FederationAPI {
    fn on_make_join<R: RoomVersion>(
        &self,
        origin: String,
        room_id: String,
        user_id: String,
    ) -> BoxFuture<FederationResult<MakeJoinResponse<R::Event>>>;

    fn on_send_join<R: RoomVersion>(
        &self,
        origin: String,
        room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>>;

    fn on_backfill<R: RoomVersion>(
        &self,
        origin: String,
        room_id: String,
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<FederationResult<BackfillResponse<R::Event>>>;

    fn on_send(
        &self,
        origin: String,
        txn: TransactionRequest,
    ) -> BoxFuture<FederationResult<TransactionResponse>>;
}

#[derive(Serialize)]
pub struct MakeJoinResponse<E: Serialize> {
    room_version: &'static str,
    event: E,
}

#[derive(Serialize)]
pub struct SendJoinResponse<E: Serialize> {
    origin: String,
    state: Vec<E>,
    auth_chain: Vec<E>,
}

#[derive(Serialize)]
pub struct BackfillResponse<E: Serialize> {
    pdus: Vec<E>,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionRequest {
    pdus: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub struct TransactionResponse;

pub struct AuthHeader<'a> {
    pub origin: &'a str,
    pub key_id: &'a str,
    pub signature: &'a str,
}

pub fn parse_auth_header(header: &str) -> Option<AuthHeader> {
    // X-Matrix origin={},key="{}",sig="{}"

    let header = header.strip_prefix("X-Matrix ")?;

    let mut origin = None;
    let mut key_id = None;
    let mut signature = None;
    for item in header.split(',') {
        let (key, value) = item.split_at(item.find('=')?);
        let value = value.trim_matches('=');

        // Strip out any quotes.
        let value = if value.starts_with('"') && value.ends_with('"') {
            &value[1..value.len() - 1]
        } else {
            value
        };

        match key {
            "origin" => origin = Some(value),
            "key" => key_id = Some(value),
            "sig" => signature = Some(value),
            _ => {}
        }
    }

    Some(AuthHeader {
        origin: origin?,
        key_id: key_id?,
        signature: signature?,
    })
}

#[test]
fn test_parse_auth_header() {
    let header = parse_auth_header(
        r#"X-Matrix origin=foo.com,key="key_id",sig="some_signature""#,
    )
    .unwrap();

    assert_eq!(header.origin, "foo.com");
    assert_eq!(header.key_id, "key_id");
    assert_eq!(header.signature, "some_signature");
}
