use bytes::Bytes;
use serde::de::Deserialize;
use serde::ser::Serialize;


pub fn bool_to_bytes(b: &bool) -> Bytes {
    if *b {
        Bytes::from_static(&[1u8])
    } else {
        Bytes::from_static(&[0u8])
    }
}

pub fn bytes_to_bool(bytes: &Bytes) -> bool {
    bytes[0] == 1u8
}

pub fn json_to_bytes<T>(json: &T) -> Result<Bytes, serde_json::Error>
where
    T: ?Sized + Serialize,
{
    serde_json::to_vec(json).map(|v| Bytes::from(v))
}

pub fn bytes_to_json<'a, T>(bytes: &'a Bytes) -> Result<T, serde_json::Error>
where
    T: Deserialize<'a>,
{
    serde_json::from_slice(bytes)
}
