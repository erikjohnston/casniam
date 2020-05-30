use crate::protocol::{Event, RoomState, RoomStateResolver, RoomVersion};
use crate::stores::EventStore;

use bb8::{Pool, PooledConnection};
use bb8_postgres::tokio_postgres::NoTls;
use bb8_postgres::PostgresConnectionManager;
use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json;
use std::collections::BTreeMap;

type PgPool = Pool<PostgresConnectionManager<NoTls>>;

#[derive(Debug, Clone)]
pub struct PostgresEventStore {
    pool: PgPool,
}

impl<R, S> EventStore<R, S> for PostgresEventStore
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState + Send + Sync + 'static,
    <S as IntoIterator>::IntoIter: Send,
{
    fn insert_events(
        &self,
        events: Vec<(R::Event, S)>,
    ) -> BoxFuture<Result<(), Error>> {
        async move {
            let mut connection: PooledConnection<
                PostgresConnectionManager<NoTls>,
            > = self.pool.get().await?;

            let txn = connection.transaction().await?;

            // TODO: Pipeline these queries.
            for (event, state) in events {
                txn.execute(
                    r#"INSERT INTO events (event_id, json) VALUES ($1, $2)"#,
                    &[&event.event_id(), &serde_json::to_vec(&event)?],
                )
                .await?;

                // TODO: Persist state separately.
                for ((typ, state_key), event_id) in state.into_iter() {
                    txn.execute(
                        r#"INSERT INTO state (event_id, type, state_key, state_event_id) VALUES ($1, $, $3, $4)"#,
                        &[&event.event_id(), &typ, &state_key, &event_id],
                    )
                    .await?;
                }
            }

            txn.commit().await?;

            Ok(())
        }
        .boxed()
    }

    fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<String>, Error>> {
        let event_ids: Vec<String> =
            event_ids.iter().map(|s| s.to_string()).collect();

        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            let rows = connection
                .query(
                    "SELECT event_id FROM events WHERE event_id = ANY($1)",
                    &[&event_ids],
                )
                .await?;

            let found: Vec<String> =
                rows.into_iter().map(|row| row.get(0)).collect();

            Ok(event_ids
                .into_iter()
                .filter(|event_id| found.contains(event_id))
                .collect())
        }
        .boxed()
    }

    fn get_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        let event_ids: Vec<String> =
            event_ids.iter().map(|s| s.to_string()).collect();

        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            let rows = connection
                .query(
                    "SELECT json FROM events WHERE event_id = ANY($1)",
                    &[&event_ids],
                )
                .await?;

            let mut events = Vec::with_capacity(event_ids.len());
            for row in rows {
                let row: Vec<u8> = row.get(0);
                let event: R::Event = serde_json::from_slice(&row).unwrap();
                events.push(event);
            }

            Ok(events)
        }
        .boxed()
    }

    fn get_state_for(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Option<S>, Error>> {
        let event_ids: Vec<String> =
            event_ids.iter().map(|s| s.to_string()).collect();

        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            let rows = connection
                .query(
                    "SELECT event_id, type, state_key, state_event_id FROM state WHERE event_id = ANY($1)",
                    &[&event_ids],
                )
                .await?;

            let mut states: BTreeMap<String, S> = BTreeMap::new();

            for row in rows {
                let event_id: String = row.get(0);
                let typ: String = row.get(1);
                let state_key: String = row.get(2);
                let state_event_id: String = row.get(3);

                states.entry(event_id.clone()).or_default().add_event(typ, state_key, state_event_id);
            }

            // TODO: Return None if we don't have some state.

            let states = states.into_iter().map(|(_k, v)| v).collect();

            let state = R::State::resolve_state(states, self).await?;

            Ok(Some(state))
        }
        .boxed()
    }
}
