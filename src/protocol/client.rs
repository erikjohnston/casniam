use failure::Error;
use futures::compat::Future01CompatExt;
use futures::FutureExt;
use rand::Rng;

use std::future::Future;
use std::pin::Pin;

use crate::json::signed::Signed;
use crate::protocol::RoomVersion;

pub trait TransactionSender {
    /// Queues up an event to be sent to the given destination. Future resolves
    /// when succesfully *queued*, e.g. its been saved to a persistent queue.
    fn send_event<R: RoomVersion>(
        &self,
        destination: String,
        event: R::Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>>;
}

#[derive(Clone)]
pub struct MemoryTransactionSender {
    pub client: awc::Client,
    pub server_name: String,
    pub key_name: String,
    pub secret_key: sodiumoxide::crypto::sign::SecretKey,
}

impl TransactionSender for MemoryTransactionSender {
    fn send_event<R: RoomVersion>(
        &self,
        destination: String,
        event: R::Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
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
            method: "PUT".to_string(),
            uri: path.clone(),
            origin: self.server_name.clone(),
            destination: destination.clone(),
            content: Some(content.clone()),
        };

        let signed: Signed<_> = Signed::wrap(request_json).unwrap();
        let sig = signed.sign_detached(&self.secret_key);
        let b64_sig = base64::encode_config(&sig, base64::STANDARD_NO_PAD);

        let auth_header = format!(
            r#"X-Matrix origin={},key="{}",sig="{}""#,
            &self.server_name, &self.key_name, b64_sig,
        );

        async move {
            client
                .put(format!("https://{}{}", destination, path))
                .header("Authorization", auth_header)
                .send_json(&content)
                .compat()
                .await
                .map_err(|e| format_err!("{}", e))?;

            Ok(())
        }
        .boxed_local()
    }
}

#[derive(Serialize)]
struct RequestJson {
    method: String,
    uri: String,
    origin: String,
    destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
}
