use super::canonical::Canonical;

use std::collections::BTreeMap;

use sodiumoxide::crypto::sign::{sign_detached, SecretKey, Signature};

use serde;
use serde::de::{Deserialize, DeserializeOwned, Deserializer, Error as _};
use serde::ser::Serializer;
use serde::Serialize;

use serde_json::{Error, Value};

#[derive(Clone, Debug)]
pub struct Signed<V, U = Value> {
    value: Canonical<V>,

    signatures: BTreeMap<String, BTreeMap<String, Base64Signature>>,
    unsigned: U,
}

impl<V, U> Signed<V, U>
where
    V: Serialize,
    U: Default,
{
    pub fn wrap(value: V) -> Result<Signed<V, U>, Error> {
        Ok(Signed {
            value: Canonical::wrap(value)?,
            signatures: BTreeMap::new(),
            unsigned: U::default(),
        })
    }
}

impl<V, U> Signed<V, U> {
    pub fn add_signature(
        &mut self,
        server_name: String,
        key_name: String,
        signature: Signature,
    ) {
        self.signatures
            .entry(server_name)
            .or_default()
            .insert(key_name, Base64Signature(signature));
    }

    pub fn sign(
        &mut self,
        server_name: String,
        key_name: String,
        key: &SecretKey,
    ) {
        let sig = self.sign_detached(key);
        self.add_signature(server_name, key_name, sig);
    }

    pub fn sign_detached(&self, key: &SecretKey) -> Signature {
        sign_detached(self.get_canonical().as_bytes(), key)
    }
}

impl<V, U> AsRef<V> for Signed<V, U> {
    fn as_ref(&self) -> &V {
        self.value.as_ref()
    }
}

impl<V, U> Signed<V, U> {
    pub fn get_canonical(&self) -> &str {
        self.value.get_canonical()
    }
}

impl<'de, V, U> Deserialize<'de> for Signed<V, U>
where
    V: DeserializeOwned,
    U: DeserializeOwned + Default,
{
    fn deserialize<D>(deserializer: D) -> Result<Signed<V, U>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;

        let map = value.as_object_mut().unwrap();

        let raw_sigs = map.remove("signatures").unwrap_or_default();
        let raw_unsigned = map.remove("unsigned").unwrap_or_default();

        let signatures: BTreeMap<String, BTreeMap<String, Base64Signature>> =
            serde_json::from_value(raw_sigs)
                .map_err(serde::de::Error::custom)?;

        let unsigned: U = serde_json::from_value(raw_unsigned)
            .map_err(serde::de::Error::custom)?;

        let canonical =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;

        Ok(Signed {
            value: canonical,
            signatures,
            unsigned,
        })
    }
}

impl<V, U> Serialize for Signed<V, U>
where
    U: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut v = serde_json::to_value(&self.value)
            .map_err(serde::ser::Error::custom)?;
        let s = serde_json::to_value(&self.signatures)
            .map_err(serde::ser::Error::custom)?;
        let u = serde_json::to_value(&self.unsigned)
            .map_err(serde::ser::Error::custom)?;

        v.as_object_mut()
            .unwrap()
            .insert("signatures".to_string(), s);

        if !u.is_null() {
            v.as_object_mut().unwrap().insert("unsigned".to_string(), u);
        }

        v.serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Base64Signature(Signature);

impl From<Signature> for Base64Signature {
    fn from(sig: Signature) -> Base64Signature {
        Base64Signature(sig)
    }
}

impl From<Base64Signature> for Signature {
    fn from(sig: Base64Signature) -> Signature {
        sig.0
    }
}

impl AsRef<Signature> for Base64Signature {
    fn as_ref(&self) -> &Signature {
        &self.0
    }
}

impl serde::Serialize for Base64Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&base64::encode_config(
            &self.0,
            base64::STANDARD_NO_PAD,
        ))
    }
}

impl<'de> serde::Deserialize<'de> for Base64Signature {
    fn deserialize<D>(deserializer: D) -> Result<Base64Signature, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let de_string: String = String::deserialize(deserializer)?;

        let slice = base64::decode_config(&de_string, base64::STANDARD_NO_PAD)
            .map_err(|e| {
                D::Error::custom(format_args!(
                    "invalid base64: {}, {}",
                    de_string, e,
                ))
            })?;

        let sig = Signature::from_slice(&slice)
            .ok_or_else(|| D::Error::custom("signature incorrect length"))?;

        Ok(Base64Signature(sig))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sodiumoxide::crypto::sign;

    use serde_json;

    #[test]
    fn base64_serialize() {
        let sig_bytes = b"_k{\x8c\xdd#h\x9b\"ejy\xed\xd6\xbd\x1a\xa9\x90\xf3\xbe\x10\x15\xbb\xa4\x08\xc4\xaas\x95\\\x95\xa0~\xda~\"\xf0\xb3\xdcd9\x03\xeb\xe7\xf3\x83\x8bd~\x94\xac\x88\x80\xe8\x82F8\x1dk\xf5rq\xa1\x02";
        let sig = sign::Signature::from_slice(sig_bytes).unwrap();
        let b64 = Base64Signature(sig);
        let serialized = serde_json::to_string(&b64).unwrap();

        assert_eq!(
            serialized,
            r#""X2t7jN0jaJsiZWp57da9GqmQ874QFbukCMSqc5VclaB+2n4i8LPcZDkD6+fzg4tkfpSsiIDogkY4HWv1cnGhAg""#
        );
    }

    #[test]
    fn base64_deserialize() {
        let serialized = r#""X2t7jN0jaJsiZWp57da9GqmQ874QFbukCMSqc5VclaB+2n4i8LPcZDkD6+fzg4tkfpSsiIDogkY4HWv1cnGhAg""#;

        let sig_bytes = b"_k{\x8c\xdd#h\x9b\"ejy\xed\xd6\xbd\x1a\xa9\x90\xf3\xbe\x10\x15\xbb\xa4\x08\xc4\xaas\x95\\\x95\xa0~\xda~\"\xf0\xb3\xdcd9\x03\xeb\xe7\xf3\x83\x8bd~\x94\xac\x88\x80\xe8\x82F8\x1dk\xf5rq\xa1\x02";
        let expected_sig =
            Base64Signature(sign::Signature::from_slice(sig_bytes).unwrap());

        let de_sig: Base64Signature =
            serde_json::from_str(&serialized).unwrap();

        assert_eq!(de_sig, expected_sig);
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct A {
        a: i64,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct B {
        b: Option<i64>,
    }

    #[test]
    fn signed_deserialize() {
        let s: Signed<A> = serde_json::from_str(
            r#"{ "a": 1, "b": 2, "signatures": {}, "unsigned": {} }"#,
        )
        .unwrap();

        assert_eq!(s.value.as_ref(), &A { a: 1 });
        assert_eq!(s.signatures.len(), 0);
        assert_eq!(s.value.get_canonical(), r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn signed_serialize() {
        let s = Signed::<_, serde_json::Value>::wrap(A { a: 1 }).unwrap();

        let j = serde_json::to_string(&s).unwrap();

        assert_eq!(j, r#"{"a":1,"signatures":{}}"#);
    }

    #[test]
    fn signed_roundtrip() {
        let s: Signed<B> = serde_json::from_str(
            r#"{ "a": 1, "signatures": {}, "unsigned": {} }"#,
        )
        .unwrap();

        assert_eq!(s.value.as_ref(), &B { b: None });
        assert_eq!(s.signatures.len(), 0);
        assert_eq!(s.value.get_canonical(), r#"{"a":1}"#);

        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(j, r#"{"a":1,"signatures":{},"unsigned":{}}"#);
    }

    #[test]
    fn verify() {
        let b = r#"{"signatures": {"Alice": {"ed25519:zxcvb": "hvA+XXFEkHk80pLMeIYjNkWy5Ds2ZckSrvj00NvbyFJQe3H9LuJNnu8JLZ/ffIzChs3HmhwPldO0MSmyJAYpCA"}}, "my_key": "my_data"}"#;

        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct Test {
            my_key: String,
        };

        let s: Signed<Test> = serde_json::from_str(b).unwrap();

        assert_eq!(
            s.as_ref(),
            &Test {
                my_key: "my_data".into()
            }
        );

        let k = b"qA\xeb\xc2^+(\\~P\x91(\xa4\xf4L\x1f\xeb\x07E\xae\x8b#q(\rMq\xf2\xc9\x8f\xe1\xca";
        let seed = sign::Seed::from_slice(k).unwrap();
        let (pubkey, _) = sign::keypair_from_seed(&seed);

        let sig = &s.signatures["Alice"]["ed25519:zxcvb"];

        assert!(sign::verify_detached(
            sig.as_ref(),
            s.get_canonical().as_bytes(),
            &pubkey
        ));
    }

    #[test]
    fn sign() {
        #[derive(Debug, Serialize, PartialEq, Eq)]
        struct Test {
            my_key: String,
        };

        let mut s: Signed<Test, serde_json::value::Value> =
            Signed::wrap(Test {
                my_key: "my_data".to_string(),
            })
            .unwrap();

        assert_eq!(
            s.as_ref(),
            &Test {
                my_key: "my_data".into()
            }
        );

        let k = b"qA\xeb\xc2^+(\\~P\x91(\xa4\xf4L\x1f\xeb\x07E\xae\x8b#q(\rMq\xf2\xc9\x8f\xe1\xca";
        let seed = sign::Seed::from_slice(k).unwrap();
        let (_, privkey) = sign::keypair_from_seed(&seed);

        let sig = sign::sign_detached(s.get_canonical().as_bytes(), &privkey);
        s.signatures
            .entry("Alice".to_string())
            .or_default()
            .insert("ed25519:zxcvb".to_string(), Base64Signature(sig));

        let b = r#"{"my_key":"my_data","signatures":{"Alice":{"ed25519:zxcvb":"hvA+XXFEkHk80pLMeIYjNkWy5Ds2ZckSrvj00NvbyFJQe3H9LuJNnu8JLZ/ffIzChs3HmhwPldO0MSmyJAYpCA"}}}"#;
        assert_eq!(serde_json::to_string(&s).unwrap(), b);
    }
}
