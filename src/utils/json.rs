use bytes::Bytes;
use serde::de::Deserialize;
use serde::ser::Serialize;

pub fn json_to_bytes<T>(json: &T) -> Result<Bytes, serde_json::Error>
where
    T: ?Sized + Serialize,
{
    serde_json::to_vec(json).map(Bytes::from)
}

pub fn bytes_to_json<'a, T>(bytes: &'a Bytes) -> Result<T, serde_json::Error>
where
    T: Deserialize<'a>,
{
    serde_json::from_slice(bytes)
}

pub fn json_to_string<T>(json: &T) -> Result<String, serde_json::Error>
where
    T: ?Sized + Serialize,
{
    serde_json::to_string(json)
}

pub fn string_to_json<'a, T>(string: &'a str) -> Result<T, serde_json::Error>
where
    T: Deserialize<'a>,
{
    serde_json::from_str(string)
}
