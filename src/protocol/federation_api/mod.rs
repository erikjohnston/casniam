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
        room_id: String,
        user_id: String,
    ) -> BoxFuture<FederationResult<MakeJoinResponse<R::Event>>>;

    fn on_send_join<R: RoomVersion>(
        &self,
        room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>>;

    fn on_backfill<R: RoomVersion>(
        &self,
        room_id: String,
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<FederationResult<BackfillResponse<R::Event>>>;

    fn on_send(
        &self,
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
