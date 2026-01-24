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
        let mut saw_server = false;
        let mut saw_version = false;
        let mut saw_proto = false;
        let mut saw_id = false;
        let mut saw_mode = false;
        let mut saw_role = false;
        let mut saw_modules = false;
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
                (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "id" => {
                    assert_eq!(value, env!("CARGO_PKG_NAME"));
                    saw_id = true;
                }
                (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "mode" => {
                    assert_eq!(value, "standalone");
                    saw_mode = true;
                }
                (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "role" => {
                    assert_eq!(value, "primary");
                    saw_role = true;
                }
                (Frame::SimpleString(key), Frame::Array(Some(elements))) if key == "modules" => {
                    assert!(elements.is_empty());
                    saw_modules = true;
                }
                _ => continue,
            }
        }
        assert!(
            saw_server && saw_version && saw_proto && saw_id && saw_mode && saw_role && saw_modules
        );
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

#[test]
fn resp3_only_argument_types_rejected_before_hello() -> Result<()> {
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

    let unsupported_frames = vec![
        Frame::Boolean(true),
        Frame::Double(3.14),
        Frame::Map(Some(vec![(
            Frame::SimpleString("meta".into()),
            Frame::SimpleString("value".into()),
        )])),
        Frame::Set(Some(vec![Frame::SimpleString("member".into())])),
        Frame::Push(vec![Frame::SimpleString("element".into())]),
    ];

    for frame in unsupported_frames {
        send_array(&mut writer, vec![bulk(b"PING"), frame])?;
        let response = read_frame(&mut reader)?;
        assert!(matches!(response, Frame::Error(_)));
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn resp3_only_argument_types_rejected_after_hello() -> Result<()> {
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

    let unsupported_frames = vec![
        Frame::Map(Some(vec![(
            Frame::SimpleString("meta".into()),
            Frame::SimpleString("value".into()),
        )])),
        Frame::Set(Some(vec![Frame::SimpleString("member".into())])),
        Frame::Push(vec![Frame::SimpleString("element".into())]),
        Frame::Attribute(vec![(
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        )]),
    ];

    for frame in unsupported_frames {
        send_array(&mut writer, vec![bulk(b"PING"), frame.clone()])?;
        let response = read_frame(&mut reader)?;
        assert!(matches!(response, Frame::Error(_)));
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn resp3_scalar_arguments_allowed_after_hello() -> Result<()> {
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

    let scalar_args = vec![
        (Frame::Boolean(true), b"true".to_vec()),
        (Frame::Double(2.5), b"2.5".to_vec()),
        (
            Frame::BigNumber("9007199254740992".into()),
            b"9007199254740992".to_vec(),
        ),
        (
            Frame::VerbatimString {
                format: "txt".into(),
                payload: b"payload".to_vec(),
            },
            b"payload".to_vec(),
        ),
    ];

    for (frame, expected) in scalar_args {
        send_array(&mut writer, vec![bulk(b"PING"), frame.clone()])?;
        let response = read_frame(&mut reader)?;
        assert!(matches!(response, Frame::BulkString(Some(ref value)) if value == &expected));
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn info_command_covers_resp2_and_resp3() -> Result<()> {
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

    send_array(&mut writer, vec![bulk(b"INFO")])?;
    let info_response = read_frame(&mut reader)?;
    assert!(
        matches!(info_response, Frame::BulkString(Some(ref value)) if String::from_utf8_lossy(value).contains("ralphdb_version"))
    );

    send_array(&mut writer, vec![bulk(b"INFO"), bulk(b"unknown")])?;
    let error_response = read_frame(&mut reader)?;
    assert!(
        matches!(error_response, Frame::Error(ref message) if message.contains("unsupported INFO section"))
    );

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(&mut writer, vec![bulk(b"INFO")])?;
    let info_resp3 = read_frame(&mut reader)?;
    assert!(
        matches!(info_resp3, Frame::BulkString(Some(ref value)) if String::from_utf8_lossy(value).contains("ralphdb_version"))
    );

    send_array(&mut writer, vec![bulk(b"INFO"), bulk(b"unknown")])?;
    let error_resp3 = read_frame(&mut reader)?;
    assert!(
        matches!(error_resp3, Frame::Error(ref message) if message.contains("unsupported INFO section"))
    );

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn client_setname_supported_in_both_protocols() -> Result<()> {
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

    send_array(
        &mut writer,
        vec![bulk(b"CLIENT"), bulk(b"SETNAME"), bulk(b"default")],
    )?;
    let first_response = read_frame(&mut reader)?;
    assert!(matches!(first_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(
        &mut writer,
        vec![bulk(b"CLIENT"), bulk(b"SETNAME"), bulk(b"resp3")],
    )?;
    let second_response = read_frame(&mut reader)?;
    assert!(matches!(second_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn client_getname_reports_value_across_protocols() -> Result<()> {
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

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"GETNAME")])?;
    let first_response = read_frame(&mut reader)?;
    assert!(matches!(first_response, Frame::BulkString(None)));

    send_array(
        &mut writer,
        vec![bulk(b"CLIENT"), bulk(b"SETNAME"), bulk(b"client-one")],
    )?;
    let set_response = read_frame(&mut reader)?;
    assert!(matches!(set_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"GETNAME")])?;
    let second_response = read_frame(&mut reader)?;
    assert!(
        matches!(second_response, Frame::BulkString(Some(ref value)) if value == b"client-one")
    );

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"GETNAME")])?;
    let third_response = read_frame(&mut reader)?;
    assert!(matches!(third_response, Frame::BulkString(Some(ref value)) if value == b"client-one"));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn client_getname_null_after_hello3() -> Result<()> {
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

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"GETNAME")])?;
    let response = read_frame(&mut reader)?;
    assert!(matches!(response, Frame::Null));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn client_list_produces_attribute_push_integration() -> Result<()> {
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
        vec![bulk(b"CLIENT"), bulk(b"SETNAME"), bulk(b"integration")],
    )?;
    let set_response = read_frame(&mut reader)?;
    assert!(matches!(set_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"LIST")])?;
    let list_response = read_frame(&mut reader)?;
    if let Frame::Attribute(attributes) = list_response {
        assert_eq!(attributes.len(), 1);
        let (_, value) = &attributes[0];
        if let Frame::Push(elements) = value {
            assert_eq!(elements.len(), 1);
            if let Frame::Map(Some(entries)) = &elements[0] {
                let mut saw_name = false;
                for (entry_key, entry_value) in entries {
                    if let Frame::SimpleString(entry_key) = entry_key {
                        if entry_key == "name" {
                            assert!(matches!(
                                entry_value,
                                Frame::BulkString(Some(bytes)) if bytes == b"integration"
                            ));
                            saw_name = true;
                        }
                    }
                }
                assert!(saw_name);
            } else {
                panic!("CLIENT LIST push entry should be a map");
            }
        } else {
            panic!("CLIENT LIST attribute should wrap a push");
        }
    } else {
        panic!("CLIENT LIST RESP3 should return attributes");
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn config_get_star_available() -> Result<()> {
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

    send_array(&mut writer, vec![bulk(b"CONFIG"), bulk(b"GET"), bulk(b"*")])?;
    let response = read_frame(&mut reader)?;
    if let Frame::Array(Some(entries)) = response {
        assert!(entries.len() % 2 == 0);
        let keys: Vec<_> = entries
            .chunks(2)
            .filter_map(|pair| {
                if let [Frame::BulkString(Some(key)), _] = pair {
                    Some(String::from_utf8_lossy(key).to_string())
                } else {
                    None
                }
            })
            .collect();
        assert!(keys.contains(&"server.name".to_string()));
        assert!(keys.contains(&"server.version".to_string()));
    } else {
        panic!("CONFIG GET * should respond with an array");
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}
