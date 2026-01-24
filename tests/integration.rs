use std::{
    io::{BufReader, BufWriter, Write},
    net::{TcpListener, TcpStream},
    thread,
};

use anyhow::Result;

use ralphdb::{
    protocol::{self, Frame, ProtocolVersion},
    server::Server,
    storage::Storage,
};

fn send_array(writer: &mut BufWriter<TcpStream>, elements: Vec<Frame>) -> Result<()> {
    let frame = Frame::Array(Some(elements));
    protocol::encode_frame(&frame, ProtocolVersion::Resp3, writer)?;
    writer.flush()?;
    Ok(())
}

fn read_frame(reader: &mut BufReader<TcpStream>) -> Result<Frame> {
    Ok(protocol::decode_frame(reader)?)
}

fn bulk(value: &[u8]) -> Frame {
    Frame::BulkString(Some(value.to_vec()))
}

fn assert_hello3_map(frame: Frame) {
    if let Frame::Map(Some(entries)) = frame {
        assert_eq!(entries.len(), 3);
        let mut saw_server = false;
        let mut saw_version = false;
        let mut saw_proto = false;
        for (key, value) in entries {
            match (key, value) {
                (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "server" => {
                    assert_eq!(value, "ralphdb");
                    saw_server = true;
                }
                (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "version" => {
                    assert_eq!(value, env!("CARGO_PKG_VERSION"));
                    saw_version = true;
                }
                (Frame::SimpleString(key), Frame::Integer(value)) if key == "proto" => {
                    assert_eq!(value, 3);
                    saw_proto = true;
                }
                _ => continue,
            }
        }
        assert!(saw_server && saw_version && saw_proto);
    } else {
        panic!("HELLO 3 should respond with a map");
    }
}

#[test]
fn tcp_command_flow() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(
        &mut writer,
        vec![bulk(b"SET"), bulk(b"key"), bulk(b"value")],
    )?;
    let set_response = read_frame(&mut reader)?;
    assert!(matches!(set_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"key")])?;
    let get_response = read_frame(&mut reader)?;
    assert!(matches!(get_response, Frame::BulkString(Some(ref value)) if value == b"value"));

    send_array(&mut writer, vec![bulk(b"DEL"), bulk(b"key")])?;
    let del_response = read_frame(&mut reader)?;
    assert!(matches!(del_response, Frame::Integer(1)));

    send_array(&mut writer, vec![bulk(b"PING")])?;
    let ping_response = read_frame(&mut reader)?;
    assert!(matches!(ping_response, Frame::SimpleString(ref value) if value == "PONG"));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_response = read_frame(&mut reader)?;
    assert!(matches!(quit_response, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;

    Ok(())
}

#[test]
fn null_semantics_follow_protocol() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"missing")])?;
    let first_get = read_frame(&mut reader)?;
    assert!(matches!(first_get, Frame::BulkString(None)));

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"missing")])?;
    let second_get = read_frame(&mut reader)?;
    assert!(matches!(second_get, Frame::Null));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;

    Ok(())
}
