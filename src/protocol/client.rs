use futures::FutureExt;
use rand::Rng;

use std::future::Future;
use std::pin::Pin;

use crate::protocol::RoomVersion;

pub trait TransactionSender {
    /// Queues up an event to be sent to the given destination. Future resolves
    /// when succesfully *queued*, e.g. its been saved to a persistent queue.
    fn send_event<R: RoomVersion>(
        &self,
        destination: String,
        event: R::Event,
    ) -> Pin<Box<dyn Future<Output = ()>>>;
}

pub struct MemoryTransactionSender {
    client: awc::Client,
}

impl TransactionSender for MemoryTransactionSender {
    fn send_event<R: RoomVersion>(
        &self,
        destination: String,
        event: R::Event,
    ) -> Pin<Box<dyn Future<Output = ()>>> {
        let mut rng = rand::thread_rng();
        let txn_id: String = std::iter::repeat(())
            .map(|()| rng.sample(rand::distributions::Alphanumeric))
            .take(7)
            .collect();

        let path = format!("/_matrix/federation/v1/send/{}", txn_id);

        futures::compat::Compat::new(
            self.client
                .put(format!("https://{}{}", destination, path))
                .send_json(&event),
        )
        .boxed_local()
    }
}
