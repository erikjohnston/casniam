use anymap::any::Any;
use futures_util::future::FutureExt;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::state_map::StateMap;
use crate::stores::{EventStore, RoomStore};

use super::*;

pub struct StandardFederationAPI<S> {
    stores: S,
}

impl<S> FederationAPI for StandardFederationAPI<S> {
    fn on_make_join<R: RoomVersion>(
        &self,
        room_id: String,
        user_id: String,
    ) -> BoxFuture<FederationResult<MakeJoinResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_send_join<R: RoomVersion>(
        &self,
        room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_backfill<R: RoomVersion>(
        &self,
        room_id: String,
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<FederationResult<BackfillResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_send(
        &self,
        txn: TransactionRequest,
    ) -> BoxFuture<FederationResult<TransactionResponse>> {
        unimplemented!()
    }
}
