use std::io::{self, BufRead, Write};
use std::str::FromStr;

const MAX_BULK_SIZE: i64 = 32 * 1024 * 1024; // 32 MiB per bulk string
const MAX_COLLECTION_SIZE: i64 = 1_000_000; // 1 million entries per collection

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ProtocolVersion {
    #[default]
    Resp2,
    Resp3,
}

#[derive(Clone, Debug)]
pub enum Frame {
    SimpleString(String),
    Error(String),
    Integer(i64),
    BulkString(Option<Vec<u8>>),
    Array(Option<Vec<Frame>>),
    Null,
    Boolean(bool),
    Double(f64),
    Map(Option<Vec<(Frame, Frame)>>),
    Set(Option<Vec<Frame>>),
    Push(Vec<Frame>),
    Attribute(Vec<(Frame, Frame)>),
    VerbatimString { format: String, payload: Vec<u8> },
    BigNumber(String),
}

fn read_line_bytes<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let len = reader.read_until(b'\n', &mut buffer)?;
    if len == 0 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }

    if buffer.ends_with(b"\r\n") {
        buffer.truncate(buffer.len() - 2);
        Ok(buffer)
    } else if buffer.ends_with(b"\n") {
        buffer.truncate(buffer.len() - 1);
        Ok(buffer)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing CRLF in frame",
        ))
    }
}

fn read_line<R: BufRead>(reader: &mut R) -> io::Result<String> {
    let bytes = read_line_bytes(reader)?;
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8 in frame"))
}

fn parse_length(line: &str) -> io::Result<i64> {
    i64::from_str(line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid length value"))
}

fn ensure_non_negative(length: i64, max: i64, kind: &str) -> io::Result<usize> {
    if length < 0 {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{kind} must be non-negative"),
        ))
    } else if length > max {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{kind} exceeds maximum allowed size"),
        ))
    } else {
        Ok(length as usize)
    }
}

fn ensure_collection_length(length: i64) -> io::Result<usize> {
    ensure_non_negative(length, MAX_COLLECTION_SIZE, "collection length")
}

fn ensure_bulk_length(length: i64) -> io::Result<usize> {
    ensure_non_negative(length, MAX_BULK_SIZE, "bulk length")
}

pub fn decode_frame<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let mut prefix = [0u8];
    reader.read_exact(&mut prefix)?;
    match prefix[0] {
        b'+' => {
            let line = read_line(reader)?;
            Ok(Frame::SimpleString(line))
        }
        b'-' => {
            let line = read_line(reader)?;
            Ok(Frame::Error(line))
        }
        b':' => {
            let line = read_line(reader)?;
            let value = i64::from_str(&line)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid integer frame"))?;
            Ok(Frame::Integer(value))
        }
        b'$' => decode_bulk(reader),
        b'*' => decode_array(reader),
        b'#' => {
            let line = read_line(reader)?;
            match line.as_str() {
                "t" => Ok(Frame::Boolean(true)),
                "f" => Ok(Frame::Boolean(false)),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid boolean",
                )),
            }
        }
        b',' => {
            let line = read_line(reader)?;
            let value = f64::from_str(&line)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid double frame"))?;
            Ok(Frame::Double(value))
        }
        b'_' => {
            let line = read_line(reader)?;
            if line.is_empty() {
                Ok(Frame::Null)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid null frame",
                ))
            }
        }
        b'%' => decode_map(reader),
        b'~' => decode_set(reader),
        b'>' => decode_push(reader),
        b'|' => decode_attribute(reader),
        b'=' => decode_verbatim(reader),
        b'(' => decode_bignum(reader),
        other => decode_inline_frame(other, reader),
    }
}

fn decode_inline_frame<R: BufRead>(first_byte: u8, reader: &mut R) -> io::Result<Frame> {
    let mut remaining = read_line_bytes(reader)?;
    let mut line = Vec::with_capacity(1 + remaining.len());
    line.push(first_byte);
    line.append(&mut remaining);

    let tokens: Vec<_> = line
        .split(|byte| *byte == b' ' || *byte == b'\t')
        .filter(|token| !token.is_empty())
        .map(|token| Frame::BulkString(Some(token.to_vec())))
        .collect();

    if tokens.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty inline command",
        ));
    }

    Ok(Frame::Array(Some(tokens)))
}

fn decode_bulk<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = parse_length(&line)?;
    if length == -1 {
        return Ok(Frame::BulkString(None));
    }

    if length < -1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid bulk length",
        ));
    }

    let length = ensure_bulk_length(length)?;
    let mut buffer = vec![0u8; length];
    reader.read_exact(&mut buffer)?;
    let mut crlf = [0u8; 2];
    reader.read_exact(&mut crlf)?;
    if crlf != [b'\r', b'\n'] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing CRLF after bulk data",
        ));
    }

    Ok(Frame::BulkString(Some(buffer)))
}

fn decode_array<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = parse_length(&line)?;
    if length == -1 {
        return Ok(Frame::Array(None));
    }

    if length < -1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid array length",
        ));
    }

    let count = ensure_collection_length(length)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(decode_frame(reader)?);
    }
    Ok(Frame::Array(Some(items)))
}

fn decode_map<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = parse_length(&line)?;
    if length == -1 {
        return Ok(Frame::Map(None));
    }

    if length < -1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid map length",
        ));
    }

    let pairs = ensure_collection_length(length)?;
    let mut entries = Vec::with_capacity(pairs);
    for _ in 0..pairs {
        let key = decode_frame(reader)?;
        let value = decode_frame(reader)?;
        entries.push((key, value));
    }

    Ok(Frame::Map(Some(entries)))
}

fn decode_set<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = parse_length(&line)?;
    if length == -1 {
        return Ok(Frame::Set(None));
    }

    if length < -1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid set length",
        ));
    }

    let count = ensure_collection_length(length)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(decode_frame(reader)?);
    }

    Ok(Frame::Set(Some(items)))
}

fn decode_push<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = parse_length(&line)?;
    if length < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid push length",
        ));
    }

    let count = ensure_collection_length(length)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(decode_frame(reader)?);
    }

    Ok(Frame::Push(items))
}

fn decode_attribute<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = parse_length(&line)?;
    if length < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid attribute length",
        ));
    }

    let pairs = ensure_collection_length(length)?;
    let mut attributes = Vec::with_capacity(pairs);
    for _ in 0..pairs {
        let key = decode_frame(reader)?;
        let value = decode_frame(reader)?;
        attributes.push((key, value));
    }

    Ok(Frame::Attribute(attributes))
}

fn decode_verbatim<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let length_line = read_line(reader)?;
    let total_length = parse_length(&length_line)?;
    if total_length < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "verbatim length must be non-negative",
        ));
    }

    let total_length = ensure_bulk_length(total_length)?;
    if total_length < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "verbatim payload too short",
        ));
    }

    let mut buffer = vec![0u8; total_length];
    reader.read_exact(&mut buffer)?;
    let mut crlf = [0u8; 2];
    reader.read_exact(&mut crlf)?;
    if crlf != [b'\r', b'\n'] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing CRLF after verbatim payload",
        ));
    }

    let format_bytes = &buffer[..3];
    if buffer[3] != b':' {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing colon after verbatim format",
        ));
    }

    let format = String::from_utf8(format_bytes.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid verbatim format tag"))?;
    if !format.is_ascii() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid verbatim format tag",
        ));
    }
    let payload = buffer[4..].to_vec();

    Ok(Frame::VerbatimString { format, payload })
}

fn decode_bignum<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty bignum payload",
        ));
    }
    Ok(Frame::BigNumber(line))
}

pub fn encode_frame<W: Write>(
    frame: &Frame,
    version: ProtocolVersion,
    writer: &mut W,
) -> io::Result<()> {
    match (frame, version) {
        (Frame::SimpleString(value), _) => write!(writer, "+{}\r\n", value)?,
        (Frame::Error(value), _) => write!(writer, "-{}\r\n", value)?,
        (Frame::Integer(value), _) => write!(writer, ":{}\r\n", value)?,
        (Frame::BulkString(Some(value)), _) => {
            write!(writer, "${}\r\n", value.len())?;
            writer.write_all(value)?;
            writer.write_all(b"\r\n")?;
        }
        (Frame::BulkString(None), _) => {
            write!(writer, "$-1\r\n")?;
        }
        (Frame::Null, ProtocolVersion::Resp3) => {
            write!(writer, "_\r\n")?;
        }
        (Frame::Null, _) => {
            write!(writer, "$-1\r\n")?;
        }
        (Frame::Boolean(value), ProtocolVersion::Resp3) => {
            write!(writer, "#{}\r\n", if *value { "t" } else { "f" })?;
        }
        (Frame::Boolean(value), _) => {
            write!(writer, ":{}\r\n", if *value { 1 } else { 0 })?;
        }
        (Frame::Double(value), ProtocolVersion::Resp3) => {
            write!(writer, ",{}\r\n", value)?;
        }
        (Frame::Double(value), _) => {
            let repr = value.to_string();
            write!(writer, "${}\r\n", repr.len())?;
            writer.write_all(repr.as_bytes())?;
            writer.write_all(b"\r\n")?;
        }
        (Frame::Array(Some(elements)), _) => {
            write!(writer, "*{}\r\n", elements.len())?;
            for item in elements {
                encode_frame(item, version, writer)?;
            }
        }
        (Frame::Array(None), _) => {
            write!(writer, "*-1\r\n")?;
        }
        (Frame::Map(Some(entries)), ProtocolVersion::Resp3) => {
            write!(writer, "%{}\r\n", entries.len())?;
            for (key, value) in entries {
                encode_frame(key, version, writer)?;
                encode_frame(value, version, writer)?;
            }
        }
        (Frame::Map(None), ProtocolVersion::Resp3) => {
            write!(writer, "%-1\r\n")?;
        }
        (Frame::Map(_), _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "map frames require RESP3",
            ));
        }
        (Frame::Set(Some(elements)), ProtocolVersion::Resp3) => {
            write!(writer, "~{}\r\n", elements.len())?;
            for item in elements {
                encode_frame(item, version, writer)?;
            }
        }
        (Frame::Set(None), ProtocolVersion::Resp3) => {
            write!(writer, "~-1\r\n")?;
        }
        (Frame::Set(_), _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "set frames require RESP3",
            ));
        }
        (Frame::Push(elements), ProtocolVersion::Resp3) => {
            write!(writer, ">{}\r\n", elements.len())?;
            for element in elements {
                encode_frame(element, version, writer)?;
            }
        }
        (Frame::Push(_), _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "push frames require RESP3",
            ));
        }
        (Frame::Attribute(attributes), ProtocolVersion::Resp3) => {
            write!(writer, "|{}\r\n", attributes.len())?;
            for (key, value) in attributes {
                encode_frame(key, version, writer)?;
                encode_frame(value, version, writer)?;
            }
        }
        (Frame::Attribute(_), _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "attribute frames require RESP3",
            ));
        }
        (Frame::VerbatimString { format, payload }, ProtocolVersion::Resp3) => {
            let format_bytes = format.as_bytes();
            if format_bytes.len() != 3 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "verbatim format tag must be exactly 3 bytes",
                ));
            }

            let total_length = format_bytes.len() + 1 + payload.len();
            write!(writer, "={}\r\n", total_length)?;
            writer.write_all(format_bytes)?;
            writer.write_all(b":")?;
            writer.write_all(payload)?;
            writer.write_all(b"\r\n")?;
        }
        (Frame::VerbatimString { .. }, _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "verbatim frames require RESP3",
            ));
        }
        (Frame::BigNumber(value), ProtocolVersion::Resp3) => {
            write!(writer, "({}\r\n", value)?;
        }
        (Frame::BigNumber(_), _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bignum frames require RESP3",
            ));
        }
    }
    Ok(())
}

pub fn encode_response<W: Write>(
    frame: &Frame,
    attributes: Option<&[(Frame, Frame)]>,
    version: ProtocolVersion,
    writer: &mut W,
) -> io::Result<()> {
    if let ProtocolVersion::Resp3 = version
        && let Some(attributes) = attributes
    {
        encode_frame(&Frame::Attribute(attributes.to_vec()), version, writer)?;
    }
    encode_frame(frame, version, writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor, Read};

    fn pseudo_random_u64(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state
    }

    #[test]
    fn parse_simple_string() {
        let mut reader = Cursor::new(b"+OK\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::SimpleString(ref value) if value == "OK"));
    }

    #[test]
    fn reject_simple_string_with_invalid_utf8() {
        let mut reader = Cursor::new(b"+\xFF\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn parse_inline_command_as_array() {
        let mut reader = Cursor::new(b"PING\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Array(Some(elements)) if elements.len() == 1));
    }

    #[test]
    fn parse_inline_command_with_arguments() {
        let mut reader = Cursor::new(b"SET key value\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Array(Some(elements)) if elements.len() == 3));
    }

    #[test]
    fn reject_empty_inline_command() {
        let mut reader = Cursor::new(b"   \r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn parse_bulk_string() {
        let mut reader = Cursor::new(b"$3\r\nfoo\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::BulkString(Some(ref value)) if value == b"foo"));
    }

    #[test]
    fn encode_integer() {
        let mut buffer = Vec::new();
        encode_frame(&Frame::Integer(42), ProtocolVersion::Resp2, &mut buffer).unwrap();
        assert_eq!(buffer, b":42\r\n");
    }

    #[test]
    fn reject_invalid_bulk_length() {
        let mut reader = Cursor::new(b"$-2\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_oversized_bulk_length() {
        let mut reader = Cursor::new(format!("${}\r\n", MAX_BULK_SIZE + 1).into_bytes());
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn parse_map_frame() {
        let mut reader = Cursor::new(b"%1\r\n+foo\r\n+bar\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Map(Some(entries)) if entries.len() == 1));
    }

    #[test]
    fn reject_truncated_composite_frames() {
        let mut map_reader = Cursor::new(b"%1\r\n+foo\r\n");
        assert!(decode_frame(&mut map_reader).is_err());

        let mut set_reader = Cursor::new(b"~2\r\n+foo\r\n");
        assert!(decode_frame(&mut set_reader).is_err());

        let mut push_reader = Cursor::new(b">2\r\n+foo\r\n");
        assert!(decode_frame(&mut push_reader).is_err());
    }

    #[test]
    fn parse_verbatim_frame() {
        let mut reader = Cursor::new(b"=9\r\ntxt:hello\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(
            matches!(frame, Frame::VerbatimString { format, payload } if format == "txt" && payload == b"hello")
        );
    }

    #[test]
    fn reject_verbatim_frame_too_short() {
        let mut reader = Cursor::new(b"=3\r\ntxt\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_verbatim_frame_missing_colon() {
        let mut reader = Cursor::new(b"=6\r\ntxtahi\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_verbatim_frame_missing_crlf() {
        let mut reader = Cursor::new(b"=6\r\ntxt:hi\rX");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_verbatim_frame_with_invalid_utf8_format() {
        let mut reader = Cursor::new(b"=6\r\n\xFF\xFF\xFF:hi\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_verbatim_frame_with_non_ascii_format() {
        let mut reader = Cursor::new(b"=7\r\n\xC2\xA2X:hi\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn parse_set_frame() {
        let mut reader = Cursor::new(b"~2\r\n+foo\r\n+bar\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Set(Some(elements)) if elements.len() == 2));
    }

    #[test]
    fn parse_push_frame() {
        let mut reader = Cursor::new(b">2\r\n+foo\r\n+bar\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Push(elements) if elements.len() == 2));
    }

    #[test]
    fn parse_attribute_frame() {
        let mut reader = Cursor::new(b"|1\r\n+foo\r\n+bar\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Attribute(attributes) if attributes.len() == 1));
    }

    #[test]
    fn parse_bignum_frame() {
        let mut reader = Cursor::new(b"(12345678901234567890\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::BigNumber(value) if value == "12345678901234567890"));
    }

    #[test]
    fn verbatim_round_trip() {
        let frame = Frame::VerbatimString {
            format: "txt".into(),
            payload: b"Some string".to_vec(),
        };
        let mut buffer = Vec::new();
        encode_frame(&frame, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"=15\r\ntxt:Some string\r\n");

        let mut reader = Cursor::new(buffer);
        let decoded = decode_frame(&mut reader).unwrap();
        assert!(
            matches!(decoded, Frame::VerbatimString { format, payload } if format == "txt" && payload == b"Some string")
        );
    }

    #[test]
    fn encode_map_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Map(Some(vec![(
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        )]));
        encode_frame(&frame, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"%1\r\n+foo\r\n+bar\r\n");
    }

    #[test]
    fn encode_null_map_resp3() {
        let mut buffer = Vec::new();
        encode_frame(&Frame::Map(None), ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"%-1\r\n");
    }

    #[test]
    fn encode_map_requires_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Map(Some(vec![(
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        )]));
        assert!(encode_frame(&frame, ProtocolVersion::Resp2, &mut buffer).is_err());
    }

    #[test]
    fn encode_set_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Set(Some(vec![
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        ]));
        encode_frame(&frame, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"~2\r\n+foo\r\n+bar\r\n");
    }

    #[test]
    fn encode_null_set_resp3() {
        let mut buffer = Vec::new();
        encode_frame(&Frame::Set(None), ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"~-1\r\n");
    }

    #[test]
    fn encode_set_requires_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Set(Some(vec![Frame::SimpleString("foo".into())]));
        assert!(encode_frame(&frame, ProtocolVersion::Resp2, &mut buffer).is_err());
    }

    #[test]
    fn encode_attribute_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Attribute(vec![(
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        )]);
        encode_frame(&frame, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"|1\r\n+foo\r\n+bar\r\n");
    }

    #[test]
    fn attribute_requires_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Attribute(vec![(
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        )]);
        assert!(encode_frame(&frame, ProtocolVersion::Resp2, &mut buffer).is_err());
    }

    #[test]
    fn encode_push_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Push(vec![
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        ]);
        encode_frame(&frame, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b">2\r\n+foo\r\n+bar\r\n");
    }

    #[test]
    fn push_requires_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::Push(vec![Frame::SimpleString("foo".into())]);
        assert!(encode_frame(&frame, ProtocolVersion::Resp2, &mut buffer).is_err());
    }

    #[test]
    fn encode_bignum_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::BigNumber("1234567890".into());
        encode_frame(&frame, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"(1234567890\r\n");
    }

    #[test]
    fn bignum_requires_resp3() {
        let mut buffer = Vec::new();
        let frame = Frame::BigNumber("123".into());
        assert!(encode_frame(&frame, ProtocolVersion::Resp2, &mut buffer).is_err());
    }

    #[test]
    fn parse_null_set_frame() {
        let mut reader = Cursor::new(b"~-1\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::Set(None)));
    }

    #[test]
    fn reject_negative_map_length() {
        let mut reader = Cursor::new(b"%-2\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_negative_set_length() {
        let mut reader = Cursor::new(b"~-2\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_negative_attribute_length() {
        let mut reader = Cursor::new(b"|-1\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_negative_push_length() {
        let mut reader = Cursor::new(b">-1\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_oversized_array_length() {
        let mut reader = Cursor::new(format!("*{}\r\n", MAX_COLLECTION_SIZE + 1).into_bytes());
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_oversized_map_length() {
        let mut reader = Cursor::new(format!("%{}\r\n", MAX_COLLECTION_SIZE + 1).into_bytes());
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_oversized_set_length() {
        let mut reader = Cursor::new(format!("~{}\r\n", MAX_COLLECTION_SIZE + 1).into_bytes());
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_oversized_push_length() {
        let mut reader = Cursor::new(format!(">{}\r\n", MAX_COLLECTION_SIZE + 1).into_bytes());
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_oversized_attribute_length() {
        let mut reader = Cursor::new(format!("|{}\r\n", MAX_COLLECTION_SIZE + 1).into_bytes());
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_invalid_integer_frame() {
        let mut reader = Cursor::new(b":notanint\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_invalid_double_frame() {
        let mut reader = Cursor::new(b",notadouble\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn reject_empty_bignum_frame() {
        let mut reader = Cursor::new(b"(\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn encode_null_resp2_as_bulk_null() {
        let mut buffer = Vec::new();
        encode_frame(&Frame::Null, ProtocolVersion::Resp2, &mut buffer).unwrap();
        assert_eq!(buffer, b"$-1\r\n");
    }

    #[test]
    fn encode_null_resp3_as_null_literal() {
        let mut buffer = Vec::new();
        encode_frame(&Frame::Null, ProtocolVersion::Resp3, &mut buffer).unwrap();
        assert_eq!(buffer, b"_\r\n");
    }

    #[test]
    fn reject_invalid_null_frame() {
        let mut reader = Cursor::new(b"_oops\r\n");
        assert!(decode_frame(&mut reader).is_err());
    }

    #[test]
    fn encode_response_emits_attributes_before_payload() {
        let attributes = vec![(
            Frame::SimpleString("meta".into()),
            Frame::SimpleString("value".into()),
        )];
        let mut buffer = Vec::new();
        encode_response(
            &Frame::SimpleString("OK".into()),
            Some(&attributes),
            ProtocolVersion::Resp3,
            &mut buffer,
        )
        .unwrap();

        let mut reader = Cursor::new(buffer);
        let first_frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(first_frame, Frame::Attribute(_)));
        let second_frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(second_frame, Frame::SimpleString(ref value) if value == "OK"));
    }

    #[test]
    fn collection_length_validator_fuzzed_limits() {
        let modulus = MAX_COLLECTION_SIZE * 3 + 1;
        let mut state = 0xdeadbeefu64;
        for _ in 0..512 {
            let candidate = (pseudo_random_u64(&mut state) % modulus as u64) as i64 - (modulus / 2);
            let valid = (0..=MAX_COLLECTION_SIZE).contains(&candidate);
            assert_eq!(ensure_collection_length(candidate).is_ok(), valid);
        }
    }

    #[test]
    fn bulk_length_validator_fuzzed_limits() {
        let modulus = MAX_BULK_SIZE * 3 + 1;
        let mut state = 0xfeedfaceu64;
        for _ in 0..512 {
            let candidate = (pseudo_random_u64(&mut state) % modulus as u64) as i64 - (modulus / 2);
            let valid = (0..=MAX_BULK_SIZE).contains(&candidate);
            assert_eq!(ensure_bulk_length(candidate).is_ok(), valid);
        }
    }

    fn assert_all_truncations_fail(frame_bytes: &[u8]) {
        for cut in 0..frame_bytes.len() {
            let mut reader = Cursor::new(&frame_bytes[..cut]);
            assert!(
                decode_frame(&mut reader).is_err(),
                "truncated frame unexpectedly parsed at cut {cut}"
            );
        }
    }

    struct ChunkedReader {
        data: Vec<u8>,
        offset: usize,
        max_chunk: usize,
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.offset >= self.data.len() {
                return Ok(0);
            }
            let remaining = self.data.len() - self.offset;
            let chunk = remaining.min(self.max_chunk).min(buf.len());
            buf[..chunk].copy_from_slice(&self.data[self.offset..self.offset + chunk]);
            self.offset += chunk;
            Ok(chunk)
        }
    }

    #[test]
    fn reject_truncated_frames_across_types() {
        assert_all_truncations_fail(b"+OK\r\n");
        assert_all_truncations_fail(b":42\r\n");
        assert_all_truncations_fail(b"$3\r\nfoo\r\n");
        assert_all_truncations_fail(b"*2\r\n+OK\r\n:1\r\n");
        assert_all_truncations_fail(b"%1\r\n+k\r\n+v\r\n");
        assert_all_truncations_fail(b"=7\r\ntxt:hi\r\n");
    }

    #[test]
    fn parse_frame_with_chunked_transport_reads() {
        let bytes = b"|1\r\n+meta\r\n+v\r\n*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n";
        for max_chunk in 1..=7 {
            let reader = ChunkedReader {
                data: bytes.to_vec(),
                offset: 0,
                max_chunk,
            };
            let mut buffered = BufReader::new(reader);

            let first = decode_frame(&mut buffered).unwrap();
            assert!(matches!(first, Frame::Attribute(attributes) if attributes.len() == 1));

            let second = decode_frame(&mut buffered).unwrap();
            assert!(matches!(second, Frame::Array(Some(items)) if items.len() == 2));
        }
    }
}
