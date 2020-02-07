use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rand::Rng;

use std::fmt::Display;

use crate::json::signed::Signed;
use crate::protocol::server_resolver::MatrixConnector;
use crate::protocol::RoomVersion;

fn enc<'a>(s: &'a str) -> impl Display + 'a {
    utf8_percent_encode(s, NON_ALPHANUMERIC)
}

#[derive(Clone)]
pub struct FederationClient {
    client: hyper::Client<MatrixConnector>,
    server_name: String,
    key_name: String,
    secret_key: sodiumoxide::crypto::sign::SecretKey,
}

impl FederationClient {
    pub async fn make_join(&self, room_id: &str, user_id: &str) {
        let path = format!(
            "/_matrix/federation/v1/make_join/{}/{}",
            enc(room_id),
            enc(user_id)
        );
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
struct RequestJson {
    method: String,
    uri: String,
    origin: String,
    destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
}
