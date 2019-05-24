use serde::de::{Deserialize, DeserializeOwned, Deserializer, Error as _};
use serde::ser::{Serialize, Serializer};
use serde_json::error::Error;
use serde_json::value::RawValue;
use std::convert::AsRef;

#[derive(Clone, Debug)]
pub struct Canonical<V> {
    value: V,
    raw_value: Box<RawValue>,
}

impl<V> Canonical<V>
where
    V: Serialize,
{
    pub fn wrap(value: V) -> Result<Canonical<V>, Error> {
        let val: serde_json::Value = serde_json::to_value(&value)?;

        let uncompact = serde_json::to_vec(&val)?;
        let raw_value = make_canonical(&uncompact)?;

        Ok(Canonical { value, raw_value })
    }
}

impl<V> Canonical<V> {
    pub fn get_canonical(&self) -> &str {
        self.raw_value.get()
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

        let raw_value = make_canonical(raw_value.get().as_bytes())
            .map_err(serde::de::Error::custom)?;

        let value = serde_json::from_str(raw_value.get())
            .map_err(serde::de::Error::custom)?;

        Ok(Canonical { value, raw_value })
    }
}

fn make_canonical(uncompact: &[u8]) -> Result<Box<RawValue>, Error> {
    let mut canonical = Vec::with_capacity(uncompact.len());
    indolentjson::compact::compact(uncompact, &mut canonical)
        .expect("Invalid JSON");

    let canonical = String::from_utf8(canonical).map_err(Error::custom)?;

    RawValue::from_string(canonical)
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

        assert_eq!(&s, r#"{"a":2,"b":3}"#);
    }
}
