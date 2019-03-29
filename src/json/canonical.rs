use serde::de::{Deserialize, DeserializeOwned, Deserializer};
use serde::ser::{Serialize, Serializer};
use serde_json::error::Error;
use serde_json::value::RawValue;
use std::convert::AsRef;

pub struct Canonical<V> {
    value: V,
    raw_value: Box<RawValue>,
}

impl<V> Canonical<V>
where
    V: Serialize,
{
    pub fn wrap(value: V) -> Result<Canonical<V>, Error> {
        let raw_value = RawValue::from_string(serde_json::to_string(&value)?)?;

        Ok(Canonical { value, raw_value })
    }
}

impl<V> Canonical<V> {
    pub fn get_str(&self) -> &str {
        self.raw_value.get()
    }

    pub fn get_canonical(&self) -> Result<Vec<u8>, Error> {
        // TODO: This is quite an inefficient way of canonicalising json

        let val: serde_json::Value = serde_json::to_value(self)?;

        // TODO: Assumes BTreeMap is serialized in key order
        let uncompact = serde_json::to_vec(&val)?;

        let mut new_vec = Vec::with_capacity(uncompact.len());
        indolentjson::compact::compact(&uncompact, &mut new_vec)
            .expect("Invalid JSON");

        Ok(new_vec)
    }
}

impl<V> AsRef<V> for Canonical<V> {
    fn as_ref(&self) -> &V {
        &self.value
    }
}

impl<V> Serialize for Canonical<V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.raw_value.serialize(serializer)
    }
}

impl<'de, V> Deserialize<'de> for Canonical<V>
where
    V: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Canonical<V>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw_value = Box::<RawValue>::deserialize(deserializer)?;
        let value = serde_json::from_str(raw_value.get())
            .map_err(serde::de::Error::custom)?;

        Ok(Canonical { value, raw_value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct A {
        a: u32,
    }

    #[test]
    fn test_deserialize() {
        let c: Canonical<A> = serde_json::from_str(r#"{"a": 2}"#).unwrap();

        assert_eq!(c.as_ref(), &A { a: 2 });
    }

    #[test]
    fn test_serialize() {
        let c = Canonical::wrap(A { a: 2 }).unwrap();

        let s = serde_json::to_string(&c).unwrap();

        assert_eq!(&s, r#"{"a":2}"#);
    }

    #[test]
    fn test_roundtrip() {
        let c: Canonical<A> =
            serde_json::from_str(r#"{"a": 2, "b": 3}"#).unwrap();
        assert_eq!(c.as_ref(), &A { a: 2 });

        let s = serde_json::to_string(&c).unwrap();

        assert_eq!(&s, r#"{"a": 2, "b": 3}"#);
    }
}
