use std::io::{self, BufRead, Write};
use std::str::FromStr;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProtocolVersion {
    Resp2,
    Resp3,
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::Resp2
    }
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
}

fn read_line<R: BufRead>(reader: &mut R) -> io::Result<String> {
    let mut buffer = Vec::new();
    let len = reader.read_until(b'\n', &mut buffer)?;
    if len == 0 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }

    if buffer.ends_with(b"\r\n") {
        buffer.truncate(buffer.len() - 2);
        Ok(String::from_utf8_lossy(&buffer).to_string())
    } else if buffer.ends_with(b"\n") {
        buffer.truncate(buffer.len() - 1);
        Ok(String::from_utf8_lossy(&buffer).to_string())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing CRLF in frame",
        ))
    }
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
            // Null value
            let _ = read_line(reader)?;
            Ok(Frame::Null)
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported frame type",
        )),
    }
}

fn decode_bulk<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let line = read_line(reader)?;
    let length = i64::from_str(&line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid bulk length"))?;
    if length == -1 {
        return Ok(Frame::BulkString(None));
    }

    let mut buffer = vec![0u8; length as usize];
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
    let length = i64::from_str(&line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid array length"))?;
    if length == -1 {
        return Ok(Frame::Array(None));
    }

    let mut items = Vec::with_capacity(length as usize);
    for _ in 0..length {
        items.push(decode_frame(reader)?);
    }
    Ok(Frame::Array(Some(items)))
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_simple_string() {
        let mut reader = Cursor::new(b"+OK\r\n");
        let frame = decode_frame(&mut reader).unwrap();
        assert!(matches!(frame, Frame::SimpleString(ref value) if value == "OK"));
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
}
