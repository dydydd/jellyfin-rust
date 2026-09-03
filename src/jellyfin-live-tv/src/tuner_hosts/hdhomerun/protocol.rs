use std::error::Error;
use std::fmt;

use jellyfin_common::Crc32;

const GET_SET_NAME: u8 = 0x03;
const GET_SET_VALUE: u8 = 0x04;
const GET_SET_LOCKKEY: u8 = 0x15;
const GET_SET_REQUEST: u16 = 0x0004;
const GET_SET_REPLY: u16 = 0x0005;
const FRAME_HEADER_LENGTH: usize = 4;
const FRAME_CRC_LENGTH: usize = 4;
const MAX_TLV_VALUE_LENGTH: usize = 0x7fff;

/// A malformed or unrepresentable HDHomeRun control packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HdHomerunProtocolError {
    TlvValueTooLong { length: usize },
    FramePayloadTooLong { length: usize },
    FrameTooShort,
    ChecksumMismatch,
    UnexpectedPacketType { actual: u16 },
    PayloadLengthMismatch { declared: usize, actual: usize },
    TruncatedTlvHeader,
    TruncatedTlvLength,
    TruncatedTlvValue { declared: usize, remaining: usize },
    MissingName,
    MissingValue,
}

impl fmt::Display for HdHomerunProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TlvValueTooLong { length } => {
                write!(formatter, "TLV value length {length} exceeds 32767 bytes")
            }
            Self::FramePayloadTooLong { length } => {
                write!(
                    formatter,
                    "frame payload length {length} exceeds 65535 bytes"
                )
            }
            Self::FrameTooShort => formatter.write_str("frame is shorter than its header and CRC"),
            Self::ChecksumMismatch => formatter.write_str("frame CRC does not match"),
            Self::UnexpectedPacketType { actual } => {
                write!(formatter, "unexpected packet type 0x{actual:04x}")
            }
            Self::PayloadLengthMismatch { declared, actual } => write!(
                formatter,
                "declared payload length {declared} does not match {actual} bytes"
            ),
            Self::TruncatedTlvHeader => formatter.write_str("truncated TLV header"),
            Self::TruncatedTlvLength => formatter.write_str("truncated variable TLV length"),
            Self::TruncatedTlvValue {
                declared,
                remaining,
            } => write!(
                formatter,
                "TLV declares {declared} bytes with only {remaining} remaining"
            ),
            Self::MissingName => formatter.write_str("get/set reply is missing its name tag"),
            Self::MissingValue => formatter.write_str("get/set reply is missing its value tag"),
        }
    }
}

impl Error for HdHomerunProtocolError {}

/// The name and value returned by an HDHomeRun get/set reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GetSetReply<'a> {
    pub name: &'a [u8],
    pub value: &'a [u8],
}

/// Encodes a variable-length, null-terminated protocol string.
///
/// # Errors
///
/// Returns [`HdHomerunProtocolError::TlvValueTooLong`] when the UTF-8 payload
/// plus its null terminator does not fit the protocol's 15-bit TLV length.
pub fn write_null_terminated_string(value: &str) -> Result<Vec<u8>, HdHomerunProtocolError> {
    let value_length =
        value
            .len()
            .checked_add(1)
            .ok_or(HdHomerunProtocolError::TlvValueTooLong {
                length: value.len(),
            })?;
    if value_length > MAX_TLV_VALUE_LENGTH {
        return Err(HdHomerunProtocolError::TlvValueTooLong {
            length: value_length,
        });
    }

    let mut encoded = Vec::with_capacity(value_length + 1 + usize::from(value_length > 127));
    write_variable_length(&mut encoded, value_length);
    encoded.extend_from_slice(value.as_bytes());
    encoded.push(0);
    Ok(encoded)
}

/// Encodes an HDHomeRun tuner property get request.
///
/// # Errors
///
/// Returns an error when a TLV or the complete frame exceeds protocol limits.
pub fn write_get_message(tuner: i32, name: &str) -> Result<Vec<u8>, HdHomerunProtocolError> {
    let mut payload = Vec::new();
    write_string_tlv(&mut payload, GET_SET_NAME, &format!("/tuner{tuner}/{name}"))?;
    finish_frame(GET_SET_REQUEST, &payload)
}

/// Encodes an HDHomeRun tuner property set request.
///
/// The lock key, when present, is encoded as a big-endian 32-bit TLV value.
///
/// # Errors
///
/// Returns an error when a TLV or the complete frame exceeds protocol limits.
pub fn write_set_message(
    tuner: i32,
    name: &str,
    value: &str,
    lockkey: Option<u32>,
) -> Result<Vec<u8>, HdHomerunProtocolError> {
    let mut payload = Vec::new();
    write_string_tlv(&mut payload, GET_SET_NAME, &format!("/tuner{tuner}/{name}"))?;
    write_string_tlv(&mut payload, GET_SET_VALUE, value)?;

    if let Some(lockkey) = lockkey {
        payload.push(GET_SET_LOCKKEY);
        write_variable_length(&mut payload, size_of::<u32>());
        payload.extend_from_slice(&lockkey.to_be_bytes());
    }

    finish_frame(GET_SET_REQUEST, &payload)
}

/// Decodes and validates an HDHomeRun get/set reply.
///
/// Unknown TLVs are skipped as required by the HDHomeRun protocol. String
/// values may omit their terminal null; the declared TLV boundary remains the
/// authoritative length in that case.
///
/// # Errors
///
/// Returns an error for invalid framing, CRC, TLV lengths, packet type, or
/// missing required name/value tags.
pub fn decode_get_set_reply(packet: &[u8]) -> Result<GetSetReply<'_>, HdHomerunProtocolError> {
    let payload = decode_frame(packet, GET_SET_REPLY)?;
    let mut tlvs = TlvIterator::new(payload);
    let mut name = None;
    let mut value = None;

    for tlv in &mut tlvs {
        let tlv = tlv?;
        match tlv.tag {
            GET_SET_NAME if name.is_none() => name = Some(without_terminal_null(tlv.value)),
            GET_SET_VALUE if name.is_some() && value.is_none() => {
                value = Some(without_terminal_null(tlv.value));
            }
            _ => {}
        }
    }

    Ok(GetSetReply {
        name: name.ok_or(HdHomerunProtocolError::MissingName)?,
        value: value.ok_or(HdHomerunProtocolError::MissingValue)?,
    })
}

/// Returns only the value from a valid get/set reply.
#[must_use]
pub fn try_get_return_value_of_get_set(packet: &[u8]) -> Option<&[u8]> {
    decode_get_set_reply(packet).ok().map(|reply| reply.value)
}

/// Checks a reply value using the ASCII case-insensitive semantics used by
/// HDHomeRun control values.
#[must_use]
pub fn verify_return_value_of_get_set(packet: &[u8], expected: &str) -> bool {
    try_get_return_value_of_get_set(packet)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected.as_bytes()))
}

fn write_string_tlv(
    output: &mut Vec<u8>,
    tag: u8,
    value: &str,
) -> Result<(), HdHomerunProtocolError> {
    output.push(tag);
    output.extend_from_slice(&write_null_terminated_string(value)?);
    Ok(())
}

fn write_variable_length(output: &mut Vec<u8>, length: usize) {
    if length <= 127 {
        output.push(length as u8);
    } else {
        output.push(((length & 0x7f) as u8) | 0x80);
        output.push((length >> 7) as u8);
    }
}

fn finish_frame(packet_type: u16, payload: &[u8]) -> Result<Vec<u8>, HdHomerunProtocolError> {
    let payload_length =
        u16::try_from(payload.len()).map_err(|_| HdHomerunProtocolError::FramePayloadTooLong {
            length: payload.len(),
        })?;
    let mut packet = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len() + FRAME_CRC_LENGTH);
    packet.extend_from_slice(&packet_type.to_be_bytes());
    packet.extend_from_slice(&payload_length.to_be_bytes());
    packet.extend_from_slice(payload);
    packet.extend_from_slice(&Crc32::compute(&packet).to_le_bytes());
    Ok(packet)
}

fn decode_frame(packet: &[u8], expected_type: u16) -> Result<&[u8], HdHomerunProtocolError> {
    if packet.len() < FRAME_HEADER_LENGTH + FRAME_CRC_LENGTH {
        return Err(HdHomerunProtocolError::FrameTooShort);
    }

    let frame_without_crc = &packet[..packet.len() - FRAME_CRC_LENGTH];
    let actual_crc = u32::from_le_bytes(
        packet[packet.len() - FRAME_CRC_LENGTH..]
            .try_into()
            .map_err(|_| HdHomerunProtocolError::FrameTooShort)?,
    );
    if actual_crc != Crc32::compute(frame_without_crc) {
        return Err(HdHomerunProtocolError::ChecksumMismatch);
    }

    let packet_type = u16::from_be_bytes([packet[0], packet[1]]);
    if packet_type != expected_type {
        return Err(HdHomerunProtocolError::UnexpectedPacketType {
            actual: packet_type,
        });
    }

    let declared_length = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    let actual_length = packet.len() - FRAME_HEADER_LENGTH - FRAME_CRC_LENGTH;
    if declared_length != actual_length {
        return Err(HdHomerunProtocolError::PayloadLengthMismatch {
            declared: declared_length,
            actual: actual_length,
        });
    }

    Ok(&packet[FRAME_HEADER_LENGTH..FRAME_HEADER_LENGTH + actual_length])
}

fn without_terminal_null(value: &[u8]) -> &[u8] {
    value.strip_suffix(&[0]).unwrap_or(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Tlv<'a> {
    tag: u8,
    value: &'a [u8],
}

struct TlvIterator<'a> {
    remaining: &'a [u8],
}

impl<'a> TlvIterator<'a> {
    const fn new(payload: &'a [u8]) -> Self {
        Self { remaining: payload }
    }
}

impl<'a> Iterator for TlvIterator<'a> {
    type Item = Result<Tlv<'a>, HdHomerunProtocolError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < 2 {
            self.remaining = &[];
            return Some(Err(HdHomerunProtocolError::TruncatedTlvHeader));
        }

        let tag = self.remaining[0];
        let first_length = self.remaining[1];
        let (length, header_length) = if first_length & 0x80 == 0 {
            (usize::from(first_length), 2)
        } else {
            if self.remaining.len() < 3 {
                self.remaining = &[];
                return Some(Err(HdHomerunProtocolError::TruncatedTlvLength));
            }
            (
                usize::from(first_length & 0x7f) | (usize::from(self.remaining[2]) << 7),
                3,
            )
        };
        let value_bytes = &self.remaining[header_length..];
        if value_bytes.len() < length {
            let remaining = value_bytes.len();
            self.remaining = &[];
            return Some(Err(HdHomerunProtocolError::TruncatedTlvValue {
                declared: length,
                remaining,
            }));
        }

        let (value, remaining) = value_bytes.split_at(length);
        self.remaining = remaining;
        Some(Ok(Tlv { tag, value }))
    }
}
