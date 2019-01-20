use r2d2::Pool;
use r2d2_postgres::{PostgresConnectionManager, TlsMode};

use failure::Error;

pub trait Event {
    fn get_id(&self) -> &str;
    fn get_serialized(&self) -> Vec<u8>;
    fn from_serialized(bytes: Vec<u8>) -> Self;
}

pub struct PostgresEventStore {
    pool: Pool<PostgresConnectionManager>,
}

impl PostgresEventStore {
    pub fn store_event(&self, event: &impl Event) -> Result<(), Error> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO events(event_id, event) VALUES ($1, $2)",
            &[&event.get_id(), &event.get_serialized()],
        )?;

        Ok(())
    }

    pub fn get_event<E: Event>(&self, id: &str) -> Result<Option<E>, Error> {
        let conn = self.pool.get()?;
        let rows =
            conn.query("SELECT event FROM events WHERE event_id = $1", &[&id])?;

        Ok(rows.iter().next().map(|row| E::from_serialized(row.get(0))))
    }
}
