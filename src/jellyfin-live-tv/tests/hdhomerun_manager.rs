use jellyfin_common::Crc32;
use jellyfin_live_tv::tuner_hosts::hdhomerun::{
    HdHomerunProtocolError, decode_get_set_reply, try_get_return_value_of_get_set,
    verify_return_value_of_get_set, write_get_message, write_null_terminated_string,
    write_set_message,
};

const VALID_REPLY: &[u8] = &[
    0, 5, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v', b'a',
    b'l', b'u', b'e', 0, 0x7d, 0xa3, 0xa3, 0xf3,
];

fn seal_reply(payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(8 + payload.len());
    packet.extend_from_slice(&5_u16.to_be_bytes());
    packet.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    packet.extend_from_slice(payload);
    packet.extend_from_slice(&Crc32::compute(&packet).to_le_bytes());
    packet
}

#[test]
fn write_null_terminated_string_empty_success() {
    assert_eq!([1, 0], write_null_terminated_string("").unwrap().as_slice());
}

#[test]
fn write_null_terminated_string_valid_success() {
    assert_eq!(
        [10, b'T', b'h', b'e', b' ', b'q', b'u', b'i', b'c', b'k', 0],
        write_null_terminated_string("The quick")
            .unwrap()
            .as_slice()
    );
}

#[test]
fn write_get_message_valid_success() {
    let expected = [
        0, 4, 0, 12, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 0xc0, 0xc9,
        0x87, 0x33,
    ];

    assert_eq!(expected, write_get_message(0, "N").unwrap().as_slice());
}

#[test]
fn write_set_message_no_lock_key_success() {
    let expected = [
        0, 4, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0xa9, 0x49, 0xd0, 0x68,
    ];

    assert_eq!(
        expected,
        write_set_message(0, "N", "value", None).unwrap().as_slice()
    );
}

#[test]
fn write_set_message_lock_key_success() {
    let expected = [
        0, 4, 0, 26, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 21, 4, 0x00, 0x01, 0x38, 0xd5, 0x8e, 0xb6, 0x06, 0x82,
    ];

    assert_eq!(
        expected,
        write_set_message(0, "N", "value", Some(80_085))
            .unwrap()
            .as_slice()
    );
}

#[test]
fn try_get_return_value_of_get_set_valid_success() {
    assert_eq!(
        Some(b"value".as_slice()),
        try_get_return_value_of_get_set(VALID_REPLY)
    );
}

#[test]
fn try_get_return_value_of_get_set_invalid_crc_false() {
    let packet = [
        0, 5, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0x7d, 0xa3, 0xa3, 0xf4,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_invalid_packet_type_false() {
    let packet = [
        0, 4, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0xa9, 0x49, 0xd0, 0x68,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_invalid_packet_false() {
    let packet = [0, 5, 0, 20, 0x7d, 0xa3, 0xa3];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_too_small_message_length_false() {
    let packet = [
        0, 5, 0, 19, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0x25, 0x25, 0x44, 0x9a,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_too_large_message_length_false() {
    let packet = [
        0, 5, 0, 21, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0xe3, 0x20, 0x79, 0x6c,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_too_large_name_length_false() {
    let packet = [
        0, 5, 0, 20, 3, 20, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0xe1, 0x8e, 0x9c, 0x74,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_invalid_get_set_name_tag_false() {
    let packet = [
        0, 5, 0, 20, 4, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0xee, 0x05, 0xe7, 0x12,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_invalid_get_set_value_tag_false() {
    let packet = [
        0, 5, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 3, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0x64, 0xaa, 0x66, 0xf9,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn try_get_return_value_of_get_set_too_large_value_length_false() {
    let packet = [
        0, 5, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 7, b'v',
        b'a', b'l', b'u', b'e', 0, 0xc9, 0xa8, 0xd4, 0x55,
    ];

    assert_eq!(None, try_get_return_value_of_get_set(&packet));
}

#[test]
fn verify_return_value_of_get_set_valid_true() {
    assert!(verify_return_value_of_get_set(VALID_REPLY, "value"));
}

#[test]
fn verify_return_value_of_get_set_wrong_value_false() {
    assert!(!verify_return_value_of_get_set(VALID_REPLY, "none"));
}

#[test]
fn verify_return_value_of_get_set_invalid_packet_false() {
    let packet = [
        0, 4, 0, 20, 3, 10, b'/', b't', b'u', b'n', b'e', b'r', b'0', b'/', b'N', 0, 4, 6, b'v',
        b'a', b'l', b'u', b'e', 0, 0x7d, 0xa3, 0xa3, 0xf3,
    ];

    assert!(!verify_return_value_of_get_set(&packet, "value"));
}

#[test]
fn variable_tlv_lengths_round_trip_beyond_one_byte() {
    let encoded = write_null_terminated_string(&"x".repeat(127)).unwrap();
    assert_eq!([0x80, 0x01], encoded[..2]);
    assert_eq!(130, encoded.len());

    let long_value = "x".repeat(127);
    let payload = [
        &[3, 2, b'N', 0][..],
        &[4, 0x80, 0x01][..],
        long_value.as_bytes(),
        &[0][..],
    ]
    .concat();
    let packet = seal_reply(&payload);
    assert_eq!(
        long_value.as_bytes(),
        decode_get_set_reply(&packet).unwrap().value
    );
}

#[test]
fn tlv_values_larger_than_the_protocol_limit_are_rejected() {
    assert_eq!(
        Err(HdHomerunProtocolError::TlvValueTooLong { length: 32_768 }),
        write_null_terminated_string(&"x".repeat(32_767))
    );
}

#[test]
fn unknown_tlvs_are_skipped_and_missing_nulls_keep_the_declared_value() {
    let payload = [
        0xfe, 1, 0xaa, 3, 1, b'N', 0xfd, 0, 4, 5, b'v', b'a', b'l', b'u', b'e', 0xfc, 1, 0xbb,
    ];
    let packet = seal_reply(&payload);

    let reply = decode_get_set_reply(&packet).unwrap();
    assert_eq!(b"N", reply.name);
    assert_eq!(b"value", reply.value);
    assert!(verify_return_value_of_get_set(&packet, "VALUE"));
}

#[test]
fn truncated_tlv_header_length_and_value_are_rejected() {
    assert_eq!(
        Err(HdHomerunProtocolError::TruncatedTlvHeader),
        decode_get_set_reply(&seal_reply(&[3]))
    );
    assert_eq!(
        Err(HdHomerunProtocolError::TruncatedTlvLength),
        decode_get_set_reply(&seal_reply(&[3, 0x80]))
    );
    assert_eq!(
        Err(HdHomerunProtocolError::TruncatedTlvValue {
            declared: 5,
            remaining: 1,
        }),
        decode_get_set_reply(&seal_reply(&[3, 5, b'N']))
    );
}
