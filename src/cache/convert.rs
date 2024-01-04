use bytes::Bytes;

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
