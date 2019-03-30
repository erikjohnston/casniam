use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use sodiumoxide::crypto::sign;

use crate::json::signed::Signed;

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerKeys {
    server_name: String,
    valid_until_ts: u64,
    verify_keys: BTreeMap<String, VerifyKey>,
    old_verify_keys: BTreeMap<String, VerifyKey>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VerifyKey {
    key: String,
}

pub fn make_server_keys(
    server_name: String,
    verify_keys: BTreeMap<String, (sign::PublicKey, sign::SecretKey)>,
    old_verify_keys: BTreeMap<String, VerifyKey>,
) -> Signed<ServerKeys> {
    let valid_until_ts =
        (Utc::now() + Duration::days(1)).timestamp_millis() as u64;

    let server_keys = ServerKeys {
        server_name: server_name.clone(),
        valid_until_ts,
        verify_keys: verify_keys
            .iter()
            .map(|(name, (pubkey, _))| {
                (
                    name.clone(),
                    VerifyKey {
                        key: base64::encode_config(
                            pubkey.as_ref(),
                            base64::STANDARD_NO_PAD,
                        ),
                    },
                )
            })
            .collect(),
        old_verify_keys,
    };

    let mut s =
        Signed::wrap(server_keys).expect("server keys to be valid json");

    for (keyname, (_, seckey)) in verify_keys {
        s.sign(server_name.clone(), keyname, &seckey);
    }

    s
}
