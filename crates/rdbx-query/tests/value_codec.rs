//! Round-trip tests for Value encoding.

use rdbx_query::value::Value;

#[test]
fn value_roundtrip() {
    for v in [
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(0),
        Value::Int(-1),
        Value::Int(i64::MAX),
        Value::Int(i64::MIN),
        Value::Float(1.25),
        Value::Float(-1.0),
        Value::Bytes(b"hello".to_vec()),
        Value::Bytes(vec![]),
    ] {
        let encoded = v.encode_value();
        let decoded = Value::decode_value(&encoded).unwrap();
        assert_eq!(decoded, v, "roundtrip failed for {v:?}");
    }
}

#[test]
fn key_encoding_is_order_preserving_for_ints() {
    let mut keys: Vec<(Vec<u8>, i64)> = Vec::new();
    for i in [-1_000_000i64, -100, -1, 0, 1, 100, 1_000_000] {
        keys.push((Value::Int(i).encode_key().unwrap(), i));
    }
    keys.sort();
    let ints: Vec<i64> = keys.iter().map(|(_, i)| *i).collect();
    assert_eq!(ints, vec![-1_000_000, -100, -1, 0, 1, 100, 1_000_000]);
}
