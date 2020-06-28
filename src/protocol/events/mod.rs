pub mod v2;
pub mod v3;

use failure::Error;
use serde_json::Value;

use crate::protocol::{Event, RoomState, RoomVersion};
use crate::stores::{EventStore, StateStore};

#[derive(Clone, Debug)]
pub struct EventBuilder {
    event_type: String,
    state_key: Option<String>,
    sender: String,
    content: serde_json::Map<String, serde_json::Value>,
    origin: String,
    origin_server_ts: u64,
    room_id: String,
    prev_events: Vec<String>,
}

impl EventBuilder {
    pub fn new(
        room_id: impl ToString,
        sender: impl ToString,
        event_type: impl ToString,
        state_key: Option<impl ToString>,
    ) -> Self {
        EventBuilder {
            origin: sender
                .to_string()
                .splitn(2, ':')
                .last()
                .expect("valid sender")
                .to_string(), // FIXME
            sender: sender.to_string(),
            event_type: event_type.to_string(),
            state_key: state_key.map(|s| s.to_string()),
            room_id: room_id.to_string(),
            prev_events: Vec::new(),
            content: serde_json::Map::new(),
            origin_server_ts: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    /// Used with json! macro.
    pub fn from_json(value: Value) -> Result<Self, Error> {
        let room_id = value
            .get("room_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format_err!("Missing room_id"))?;

        let sender = value
            .get("sender")
            .and_then(Value::as_str)
            .ok_or_else(|| format_err!("Missing sender"))?;

        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format_err!("Missing type"))?;

        let state_key = value.get("state_key").and_then(Value::as_str);

        let mut builder =
            EventBuilder::new(room_id, sender, event_type, state_key);

        if let Some(content) =
            value.get("content").and_then(Value::as_object).cloned()
        {
            builder = builder.with_content(content);
        }

        if let Some(origin) = value.get("origin").and_then(Value::as_str) {
            builder = builder.origin(origin);
        }

        if let Some(origin_server_ts) =
            value.get("origin_server_ts").and_then(Value::as_i64)
        {
            builder = builder.origin_server_ts(origin_server_ts as u64);
        }

        if let Some(prev_events) =
            value.get("prev_events").and_then(Value::as_array).cloned()
        {
            let prev_events = prev_events
                .into_iter()
                .map(|e| match e {
                    Value::String(s) => s,
                    _ => panic!("invalid prev event"),
                })
                .collect();
            builder = builder.with_prev_events(prev_events);
        }

        Ok(builder)
    }

    pub fn with_content(
        mut self,
        content: serde_json::Map<String, Value>,
    ) -> Self {
        self.content = content;
        self
    }

    pub fn origin(mut self, origin: impl ToString) -> Self {
        self.origin = origin.to_string();
        self
    }

    pub fn origin_server_ts(mut self, origin_server_ts: u64) -> Self {
        self.origin_server_ts = origin_server_ts;
        self
    }

    pub fn with_prev_events(mut self, prev_events: Vec<String>) -> Self {
        self.prev_events = prev_events;
        self
    }

    pub async fn build<
        R: RoomVersion,
        S: RoomState,
        E: EventStore<R> + ?Sized,
        SS: StateStore<R, S> + ?Sized,
    >(
        self,
        event_store: &E,
        state_store: &SS,
    ) -> Result<R::Event, Error> {
        // TODO: Only pull out a subset of the state needed.

        let state = state_store
            .get_state_after(
                &self
                    .prev_events
                    .iter()
                    .map(|e| e as &str)
                    .collect::<Vec<_>>(),
            )
            .await?
            .ok_or_else(|| {
                format_err!("No state for prev events: {:?}", &self.prev_events)
            })?;

        let prev_events = event_store
            .get_events(
                &self
                    .prev_events
                    .iter()
                    .map(|e| e as &str)
                    .collect::<Vec<_>>(),
            )
            .await?;

        let event =
            R::Event::from_builder::<R, _>(self, state, prev_events).await?;
        Ok(event)
    }
}
