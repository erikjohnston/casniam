use crate::protocol::{
    Event, RoomState, RoomStateResolver, RoomVersion, RoomVersion3,
    RoomVersion4,
};
use crate::state_map::StateMap;
use crate::stores::{
    EventStore, RoomStore, RoomVersionStore, StateStore, StoreFactory,
};

use bb8::{Pool, PooledConnection};
use bb8_postgres::tokio_postgres::NoTls;
use bb8_postgres::PostgresConnectionManager;
use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;

use std::collections::{BTreeMap, BTreeSet};

use std::sync::Arc;

type PgPool = Pool<PostgresConnectionManager<NoTls>>;

#[derive(Debug, Clone)]
pub struct PostgresEventStore {
    pool: PgPool,
}

impl PostgresEventStore {
    pub fn new(pool: PgPool) -> PostgresEventStore {
        PostgresEventStore { pool }
    }
}

impl<R> EventStore<R> for PostgresEventStore
where
    R: RoomVersion + 'static,
    R::Event: 'static,
{
    fn insert_events(
        &self,
        events: Vec<R::Event>,
    ) -> BoxFuture<Result<(), Error>> {
        async move {
            let mut connection: PooledConnection<
                PostgresConnectionManager<NoTls>,
            > = self.pool.get().await?;

            let txn = connection.transaction().await?;

            // TODO: Pipeline these queries.
            for event in events {
                txn.execute(
                    r#"INSERT INTO events (room_id, event_id, json) VALUES ($1, $2, $3)"#,
                    &[&event.room_id(), &event.event_id(), &serde_json::to_string(&event)?],
                )
                .await?;
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
                .filter(|event_id| !found.contains(event_id))
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
                let row: String = row.get(0);
                let event: R::Event = serde_json::from_str(&row).unwrap();
                events.push(event);
            }

            Ok(events)
        }
        .boxed()
    }
}

impl<R, S> StateStore<R, S> for PostgresEventStore
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState<String> + Send + Sync + 'static,
    <S as IntoIterator>::IntoIter: Send,
{
    fn insert_state(
        &self,
        event: &R::Event,
        state: S,
    ) -> BoxFuture<Result<(), Error>> {
        let event_id = event.event_id().to_string();

        let event_type = event.event_type().to_string();
        let event_state_key = event.state_key().map(str::to_string);

        async move {
            let mut connection: PooledConnection<
                PostgresConnectionManager<NoTls>,
            > = self.pool.get().await?;

            let txn = connection.transaction().await?;

            // TODO: Persist state separately.
            for ((typ, state_key), state_id) in state.into_iter() {
                txn.execute(
                    r#"INSERT INTO state (event_id, type, state_key, state_event_id) VALUES ($1, $2, $3, $4)"#,
                    &[&event_id, &typ, &state_key, &state_id],
                )
                .await?;
            }

            if let Some(state_key) = event_state_key {
                txn.execute(
                    r#"INSERT INTO state_after (event_id, type, state_key, state_event_id) VALUES ($1, $2, $3, $4)"#,
                    &[&event_id, &event_type, &state_key, &event_id],
                )
                .await?;
            }

            txn.commit().await?;

            Ok(())
        }
        .boxed()
    }

    fn get_state_before(
        &self,
        event_id: &str,
    ) -> BoxFuture<Result<Option<S>, Error>> {
        let event_id = event_id.to_string();

        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            let rows = connection
                .query(
                    "SELECT type, state_key, state_event_id FROM state WHERE event_id = $1",
                    &[&event_id],
                )
                .await?;

            let state = rows.into_iter().map(|row| {
                let typ: String = row.get(0);
                let state_key: String = row.get(1);
                let state_event_id: String = row.get(2);

                ((typ, state_key), state_event_id)
            }).collect();

            Ok(Some(state))
        }
        .boxed()
    }

    fn get_state_after(
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
                    r#"
                        SELECT event_id, type, state_key, state_event_id, 1 AS ordering FROM state WHERE event_id = ANY($1)
                        UNION
                        SELECT event_id, type, state_key, state_event_id, 2 AS ordering FROM state_after WHERE event_id = ANY($1)
                        ORDER BY ordering ASC
                    "#,
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

impl<E> RoomStore<E> for PostgresEventStore
where
    E: Event + 'static,
{
    fn insert_new_events(
        &self,
        events: Vec<E>,
    ) -> BoxFuture<Result<(), Error>> {
        async move {
            let mut connection: PooledConnection<
                PostgresConnectionManager<NoTls>,
            > = self.pool.get().await?;

            let txn = connection.transaction().await?;

            let mut room_to_events: BTreeMap<String, Vec<E>> = BTreeMap::new();
            for event in events {
                room_to_events.entry(event.room_id().to_string()).or_default().push(event);
            }

            for (room_id, events) in room_to_events {
                let rows = txn
                    .query(
                        "SELECT event_id FROM event_forward_extremities WHERE room_id = $1",
                        &[&room_id],
                    )
                    .await?;

                let mut existing_extremities: BTreeSet<String> =
                    rows.into_iter().map(|row| row.get(0)).collect();

                for event in &events {
                    existing_extremities.insert(event.event_id().to_string());
                }

                for event in &events {
                    for event_id in event.prev_event_ids() {
                        existing_extremities.remove(event_id);
                    }
                }

                // TODO: Deal with A -> B -> C

                txn.execute(
                    "DELETE FROM event_forward_extremities WHERE room_id = $1",
                    &[&room_id],
                ).await?;

                txn.execute(
                    "INSERT INTO event_forward_extremities (room_id, event_id) SELECT $1, x FROM unnest($2::text[]) x",
                    &[&room_id, &existing_extremities.into_iter().collect::<Vec<_>>()],
                ).await?;
            }

            txn.commit().await?;

            Ok(())
        }.boxed()
    }

    fn get_forward_extremities(
        &self,
        room_id: String,
    ) -> BoxFuture<Result<BTreeSet<String>, Error>> {
        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            let rows = connection
                .query(
                    "SELECT event_id FROM event_forward_extremities WHERE room_id = $1",
                    &[&room_id],
                )
                .await?;

            Ok(rows.into_iter().map(|row| row.get::<_, String>(0)).collect())
        }
        .boxed()
    }
}

impl RoomVersionStore for PostgresEventStore {
    fn get_room_version(
        &self,
        room_id: &str,
    ) -> BoxFuture<Result<Option<&'static str>, Error>> {
        let room_id = room_id.to_string();

        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            let rows = connection
                .query(
                    "SELECT version FROM room_versions WHERE room_id = $1",
                    &[&room_id],
                )
                .await?;

            if let Some(row) = rows.into_iter().next() {
                // TODO: Factor this out
                match row.get(0) {
                    RoomVersion3::VERSION => Ok(Some(RoomVersion3::VERSION)),
                    RoomVersion4::VERSION => Ok(Some(RoomVersion4::VERSION)),
                    version => bail!("Unrecognized room version {}", version),
                }
            } else {
                Ok(None)
            }
        }
        .boxed()
    }

    fn set_room_version(
        &self,
        room_id: &str,
        version: &'static str,
    ) -> BoxFuture<Result<(), Error>> {
        let room_id = room_id.to_string();
        async move {
            let connection: PooledConnection<PostgresConnectionManager<NoTls>> =
                self.pool.get().await?;

            connection.execute(
                "INSERT INTO room_versions (room_id, version) VALUES ($1, $2)",
                &[&room_id, &version]
            ).await?;

            Ok(())
        }
        .boxed()
    }
}

impl StoreFactory<StateMap<String>> for PostgresEventStore {
    fn get_event_store<R: RoomVersion>(&self) -> Arc<dyn EventStore<R>> {
        Arc::new(self.clone())
    }

    fn get_state_store<R: RoomVersion>(
        &self,
    ) -> Arc<dyn StateStore<R, StateMap<String>>> {
        Arc::new(self.clone())
    }

    fn get_room_store<R: RoomVersion>(&self) -> Arc<dyn RoomStore<R::Event>> {
        Arc::new(self.clone())
    }

    fn get_room_version_store(&self) -> Arc<dyn RoomVersionStore> {
        Arc::new(self.clone())
    }
}
