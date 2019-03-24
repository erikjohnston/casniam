use serde::ser::{Serialize, Serializer};
use serde::de::{Deserialize, Deserializer, DeserializeOwned};
use serde_json::value::RawValue;
use serde_json::error::Error;
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

        Ok(Canonical {
            value, raw_value,
        })
    }

    pub fn get_str(&self) -> &str {
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
    V: DeserializeOwned
{
    fn deserialize<D>(deserializer: D) -> Result<Canonical<V>, D::Error>
    where
        D: Deserializer<'de>,
    {

        let raw_value = Box::<RawValue>::deserialize(deserializer)?;
        let s = raw_value.get().to_string();
        let value = serde_json::from_str(&s).map_err(serde::de::Error::custom)?;

        Ok(Canonical {
            value,
            raw_value,
        })
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

        assert_eq!(c.as_ref(), &A{ a: 2 });
    }

    #[test]
    fn test_serialize() {
        let c = Canonical::wrap(A { a: 2 }).unwrap();

        let s = serde_json::to_string(&c).unwrap();

        assert_eq!(&s, r#"{"a":2}"#);
    }

    #[test]
    fn test_roundtrip() {
        let c: Canonical<A> = serde_json::from_str(r#"{"a": 2, "b": 3}"#).unwrap();
        assert_eq!(c.as_ref(), &A{ a: 2 });

        let s = serde_json::to_string(&c).unwrap();

        assert_eq!(&s, r#"{"a": 2, "b": 3}"#);
    }
}
