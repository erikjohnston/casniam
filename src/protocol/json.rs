use serde::Serialize;

pub fn serialize_canonically<T>(t: T) -> Result<Vec<u8>, serde_json::Error>
where
    T: Serialize,
{
    // First serialize to a serde_json::Value as that will ensure the keys are
    // in order.
    let val = serde_json::to_value(t)?;

    let uncompact = serde_json::to_vec(&val)?;
    let mut canonical = Vec::with_capacity(uncompact.len());
    indolentjson::compact::compact(&uncompact, &mut canonical)
        .expect("Invalid JSON");

    Ok(canonical)
}

pub fn serialize_canonically_remove_fields<T>(
    t: T,
    fields: &[&str],
) -> Result<Vec<u8>, serde_json::Error>
where
    T: Serialize,
{
    let mut val = serde_json::to_value(t)?;

    if let Some(obj) = val.as_object_mut() {
        for &field in fields {
            obj.remove(field);
        }
    }

    println!("{:#?}", val);

    let uncompact = serde_json::to_vec(&val)?;
    let mut canonical = Vec::with_capacity(uncompact.len());
    indolentjson::compact::compact(&uncompact, &mut canonical)
        .expect("Invalid JSON");

    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct A {
        b: u32,
        a: u32,
    }

    #[test]
    fn test_canonical() {
        let s = serialize_canonically(A { b: 4, a: 2 }).unwrap();
        assert_eq!(&s, br#"{"a":2,"b":4}"#);
    }

    #[test]
    fn test_canonical_remove_fields() {
        let s = serialize_canonically_remove_fields(A { b: 4, a: 2 }, &["b"])
            .unwrap();
        assert_eq!(&s, br#"{"a":2}"#);
    }
}
