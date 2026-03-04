use std::{
    collections::HashMap,
    env, fs,
    io::{BufReader, BufWriter, ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};

use ralphdb::{
    protocol::{self, Frame, ProtocolVersion},
    server::{self, Server},
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

fn wait_for_ttl_expired(
    reader: &mut BufReader<TcpStream>,
    writer: &mut BufWriter<TcpStream>,
    key: &[u8],
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        send_array(writer, vec![bulk(b"TTL"), bulk(key)])?;
        if let Frame::Integer(value) = read_frame(reader)?
            && value == -2
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(5));
    }
    Err(anyhow!(
        "key {} did not expire within timeout",
        String::from_utf8_lossy(key)
    ))
}

fn wait_for_connection_close(reader: &mut BufReader<TcpStream>, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut buffer = [0u8; 1];

    while Instant::now() < deadline {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Err(err)
                if err.kind() == ErrorKind::UnexpectedEof
                    || err.kind() == ErrorKind::ConnectionReset =>
            {
                return Ok(());
            }
            Err(err)
                if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock =>
            {
                continue;
            }
            other => {
                return Err(anyhow!(
                    "idle connection was not closed before deadline: {other:?}"
                ));
            }
        }
    }

    Err(anyhow!(
        "idle connection was not closed within {} ms",
        timeout.as_millis()
    ))
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let original = env::var(key).ok();
        match value {
            Some(value) => unsafe { env::set_var(key, value) },
            None => unsafe { env::remove_var(key) },
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            unsafe { env::set_var(self.key, value) };
        } else {
            unsafe { env::remove_var(self.key) };
        }
    }
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
                (Frame::SimpleString(key), Frame::Integer(value)) if key == "id" => {
                    assert!(value > 0);
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

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn parse_metadata(contents: &str) -> HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn find_profile_dir_for_label(label: &str) -> Result<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benchmark-results");
    let mut candidates = vec![];

    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|name| name.to_str())
            && name.ends_with(&format!("-{label}"))
        {
            candidates.push(path);
        }
    }

    candidates.sort_unstable();
    candidates
        .pop()
        .ok_or_else(|| anyhow!("no benchmark-results directory found for label {label}"))
}

#[test]
fn tcp_command_flow() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
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
fn idle_timeout_env_closes_connection() -> Result<()> {
    let _guard = EnvVarGuard::set("RALPHDB_IDLE_TIMEOUT_SECS", Some("1"));
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();
    let config = server::Config::from_env();
    assert_eq!(config.idle_timeout(), Some(Duration::from_secs(1)));
    let idle_timeout = config.idle_timeout();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, idle_timeout)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let reader_stream = stream.try_clone()?;
    reader_stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut reader = BufReader::new(reader_stream);
    wait_for_connection_close(&mut reader, Duration::from_secs(3))?;

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
        Server::handle_connection(stream, server_storage, None)?;
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
fn hello_invalid_version_rejected_and_keeps_resp2_semantics() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"NaN")])?;
    let hello_response = read_frame(&mut reader)?;
    assert!(
        matches!(hello_response, Frame::Error(ref message) if message == "ERR unsupported RESP version")
    );

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"missing")])?;
    let get_response = read_frame(&mut reader)?;
    assert!(matches!(get_response, Frame::BulkString(None)));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn hello_invalid_version_after_upgrade_keeps_resp3_semantics() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello3_response = read_frame(&mut reader)?;
    assert_hello3_map(hello3_response);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"NaN")])?;
    let invalid_response = read_frame(&mut reader)?;
    assert!(
        matches!(invalid_response, Frame::Error(ref message) if message == "ERR unsupported RESP version")
    );

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"missing")])?;
    let get_response = read_frame(&mut reader)?;
    assert!(matches!(get_response, Frame::Null));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn expire_and_ttl_resp2_flow() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(
        &mut writer,
        vec![bulk(b"SET"), bulk(b"temp-key"), bulk(b"value")],
    )?;
    read_frame(&mut reader)?;

    send_array(
        &mut writer,
        vec![bulk(b"EXPIRE"), bulk(b"temp-key"), bulk(b"1")],
    )?;
    let expire_response = read_frame(&mut reader)?;
    assert!(matches!(expire_response, Frame::Integer(1)));

    send_array(&mut writer, vec![bulk(b"TTL"), bulk(b"temp-key")])?;
    let ttl_frame = read_frame(&mut reader)?;
    if let Frame::Integer(value) = ttl_frame {
        assert!(value >= 0);
    } else {
        panic!("TTL should return integer");
    }

    wait_for_ttl_expired(&mut reader, &mut writer, b"temp-key")?;

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"temp-key")])?;
    let get_frame = read_frame(&mut reader)?;
    assert!(matches!(get_frame, Frame::BulkString(None)));

    send_array(&mut writer, vec![bulk(b"TTL"), bulk(b"temp-key")])?;
    let post_ttl = read_frame(&mut reader)?;
    assert!(matches!(post_ttl, Frame::Integer(-2)));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn expire_and_ttl_resp3_flow() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
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
        vec![bulk(b"SET"), bulk(b"shelf"), bulk(b"value")],
    )?;
    read_frame(&mut reader)?;

    send_array(
        &mut writer,
        vec![bulk(b"EXPIRE"), bulk(b"shelf"), bulk(b"1")],
    )?;
    let expire_response = read_frame(&mut reader)?;
    assert!(matches!(expire_response, Frame::Integer(1)));

    send_array(&mut writer, vec![bulk(b"TTL"), bulk(b"shelf")])?;
    let ttl_frame = read_frame(&mut reader)?;
    if let Frame::Integer(value) = ttl_frame {
        assert!(value >= 0);
    } else {
        panic!("TTL should return integer");
    }

    wait_for_ttl_expired(&mut reader, &mut writer, b"shelf")?;

    send_array(&mut writer, vec![bulk(b"GET"), bulk(b"shelf")])?;
    let get_frame = read_frame(&mut reader)?;
    assert!(matches!(get_frame, Frame::Null));

    send_array(&mut writer, vec![bulk(b"TTL"), bulk(b"shelf")])?;
    let post_ttl = read_frame(&mut reader)?;
    assert!(matches!(post_ttl, Frame::Integer(-2)));

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn incr_decr_resp2_flow() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(
        &mut writer,
        vec![bulk(b"SET"), bulk(b"counter"), bulk(b"10")],
    )?;
    let set_response = read_frame(&mut reader)?;
    assert!(matches!(set_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"INCR"), bulk(b"counter")])?;
    let incr_response = read_frame(&mut reader)?;
    assert!(matches!(incr_response, Frame::Integer(11)));

    send_array(&mut writer, vec![bulk(b"DECR"), bulk(b"counter")])?;
    let decr_response = read_frame(&mut reader)?;
    assert!(matches!(decr_response, Frame::Integer(10)));

    send_array(
        &mut writer,
        vec![bulk(b"SET"), bulk(b"counter"), bulk(b"NaN")],
    )?;
    read_frame(&mut reader)?;

    send_array(&mut writer, vec![bulk(b"INCR"), bulk(b"counter")])?;
    let invalid_response = read_frame(&mut reader)?;
    assert!(
        matches!(invalid_response, Frame::Error(ref message) if message.contains("value is not an integer"))
    );

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn incr_decr_resp3_flow() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
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
        vec![bulk(b"SET"), bulk(b"counter"), bulk(b"5")],
    )?;
    read_frame(&mut reader)?;

    send_array(&mut writer, vec![bulk(b"INCR"), bulk(b"counter")])?;
    let incr_response = read_frame(&mut reader)?;
    assert!(matches!(incr_response, Frame::Integer(6)));

    send_array(&mut writer, vec![bulk(b"DECR"), bulk(b"counter")])?;
    let decr_response = read_frame(&mut reader)?;
    assert!(matches!(decr_response, Frame::Integer(5)));

    send_array(
        &mut writer,
        vec![bulk(b"SET"), bulk(b"counter"), bulk(b"Nope")],
    )?;
    read_frame(&mut reader)?;

    send_array(&mut writer, vec![bulk(b"INCR"), bulk(b"counter")])?;
    let invalid_response = read_frame(&mut reader)?;
    assert!(
        matches!(invalid_response, Frame::Error(ref message) if message.contains("value is not an integer"))
    );

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn incr_is_atomic_under_multi_client_contention() -> Result<()> {
    const CLIENT_COUNT: usize = 4;
    const INCREMENTS_PER_CLIENT: usize = 100;
    const KEY: &[u8] = b"contended-counter";

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let mut connection_handles = Vec::with_capacity(CLIENT_COUNT + 1);
        for _ in 0..(CLIENT_COUNT + 1) {
            let (stream, _) = listener.accept()?;
            let connection_storage = server_storage.clone();
            connection_handles.push(thread::spawn(move || -> Result<()> {
                Server::handle_connection(stream, connection_storage, None)
            }));
        }

        for connection_handle in connection_handles {
            connection_handle
                .join()
                .expect("connection thread panicked")?;
        }
        Ok(())
    });

    let mut client_handles = Vec::with_capacity(CLIENT_COUNT);
    for _ in 0..CLIENT_COUNT {
        client_handles.push(thread::spawn(move || -> Result<()> {
            let stream = TcpStream::connect(addr)?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut writer = BufWriter::new(stream);

            for _ in 0..INCREMENTS_PER_CLIENT {
                send_array(&mut writer, vec![bulk(b"INCR"), bulk(KEY)])?;
                let response = read_frame(&mut reader)?;
                assert!(matches!(response, Frame::Integer(_)));
            }

            send_array(&mut writer, vec![bulk(b"QUIT")])?;
            let quit_response = read_frame(&mut reader)?;
            assert!(matches!(quit_response, Frame::SimpleString(ref value) if value == "OK"));
            Ok(())
        }));
    }

    for client_handle in client_handles {
        client_handle.join().expect("client thread panicked")?;
    }

    let expected = (CLIENT_COUNT * INCREMENTS_PER_CLIENT).to_string();
    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"GET"), bulk(KEY)])?;
    let get_response = read_frame(&mut reader)?;
    assert!(
        matches!(get_response, Frame::BulkString(Some(ref value)) if value == expected.as_bytes())
    );

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
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    let unsupported_frames = vec![
        Frame::Boolean(true),
        Frame::Double(std::f64::consts::PI),
        Frame::BigNumber("9007199254740992".into()),
        Frame::VerbatimString {
            format: "txt".into(),
            payload: b"payload".to_vec(),
        },
        Frame::Map(Some(vec![(
            Frame::SimpleString("meta".into()),
            Frame::SimpleString("value".into()),
        )])),
        Frame::Set(Some(vec![Frame::SimpleString("member".into())])),
        Frame::Attribute(vec![(
            Frame::SimpleString("foo".into()),
            Frame::SimpleString("bar".into()),
        )]),
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
        Server::handle_connection(stream, server_storage, None)?;
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
        Server::handle_connection(stream, server_storage, None)?;
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
fn resp3_scalar_arguments_rejected_after_hello2_downgrade() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello3_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello3_frame);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"2")])?;
    let hello2_frame = read_frame(&mut reader)?;
    assert!(matches!(hello2_frame, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"PING"), Frame::Boolean(true)])?;
    let ping_response = read_frame(&mut reader)?;
    assert!(matches!(ping_response, Frame::Error(_)));

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
        Server::handle_connection(stream, server_storage, None)?;
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
        Server::handle_connection(stream, server_storage, None)?;
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
        Server::handle_connection(stream, server_storage, None)?;
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
        Server::handle_connection(stream, server_storage, None)?;
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
        Server::handle_connection(stream, server_storage, None)?;
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
    let attribute_frame = read_frame(&mut reader)?;
    if let Frame::Attribute(attributes) = attribute_frame {
        assert_eq!(attributes.len(), 1);
        let (_, value) = &attributes[0];
        if let Frame::Push(elements) = value {
            assert_eq!(elements.len(), 1);
            if let Frame::Map(Some(entries)) = &elements[0] {
                let mut saw_name = false;
                for (entry_key, entry_value) in entries {
                    if let Frame::SimpleString(entry_key) = entry_key
                        && entry_key == "name"
                    {
                        assert!(matches!(
                            entry_value,
                            Frame::BulkString(Some(bytes)) if bytes == b"integration"
                        ));
                        saw_name = true;
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

    let summary_frame = read_frame(&mut reader)?;
    if let Frame::SimpleString(summary) = summary_frame {
        assert!(summary.contains("protocol=RESP3"));
        assert!(summary.contains("name=integration"));
    } else {
        panic!("CLIENT LIST RESP3 should send a summary string after attributes");
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn client_list_resp2_summary_integration() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"LIST")])?;
    if let Frame::SimpleString(summary) = read_frame(&mut reader)? {
        assert!(summary.contains("protocol=RESP2"));
        assert!(summary.contains("name=(null)"));
    } else {
        panic!("RESP2 CLIENT LIST should return a summary string");
    }

    send_array(
        &mut writer,
        vec![bulk(b"CLIENT"), bulk(b"SETNAME"), bulk(b"resp2-client")],
    )?;
    let set_response = read_frame(&mut reader)?;
    assert!(matches!(set_response, Frame::SimpleString(ref value) if value == "OK"));

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"LIST")])?;
    if let Frame::SimpleString(summary) = read_frame(&mut reader)? {
        assert!(summary.contains("protocol=RESP2"));
        assert!(summary.contains("name=resp2-client"));
    } else {
        panic!("RESP2 CLIENT LIST should return a summary string");
    }

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn client_id_stable_and_monotonic() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        for _ in 0..2 {
            let (stream, _) = listener.accept()?;
            Server::handle_connection(stream, server_storage.clone(), None)?;
        }
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"ID")])?;
    let first_id_frame = read_frame(&mut reader)?;
    let first_id = if let Frame::Integer(value) = first_id_frame {
        value
    } else {
        panic!("CLIENT ID should return integer");
    };

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"ID")])?;
    let second_id_frame = read_frame(&mut reader)?;
    let second_id = if let Frame::Integer(value) = second_id_frame {
        value
    } else {
        panic!("CLIENT ID should return integer");
    };
    assert_eq!(second_id, first_id);

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(&mut writer, vec![bulk(b"CLIENT"), bulk(b"ID")])?;
    let third_id_frame = read_frame(&mut reader)?;
    let third_id = if let Frame::Integer(value) = third_id_frame {
        value
    } else {
        panic!("CLIENT ID should return integer");
    };
    assert!(third_id > first_id);

    send_array(&mut writer, vec![bulk(b"QUIT")])?;
    let quit_frame = read_frame(&mut reader)?;
    assert!(matches!(quit_frame, Frame::SimpleString(ref value) if value == "OK"));

    handle.join().expect("server thread panicked")?;
    Ok(())
}

#[test]
fn config_get_exact_key_and_nonmatching_patterns() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        let (stream, _) = listener.accept()?;
        Server::handle_connection(stream, server_storage, None)?;
        Ok(())
    });

    let stream = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);

    let check = |frame: Frame, expect_key: Option<&'static str>| {
        if let Frame::Array(Some(elements)) = frame {
            if let Some(expected) = expect_key {
                assert_eq!(elements.len(), 2);
                if let Frame::BulkString(Some(key_bytes)) = &elements[0] {
                    assert_eq!(expected, String::from_utf8_lossy(key_bytes));
                } else {
                    panic!("CONFIG GET key should be bulk string");
                }
                if let Frame::BulkString(Some(value_bytes)) = &elements[1] {
                    assert!(!value_bytes.is_empty());
                } else {
                    panic!("CONFIG GET value should be bulk string");
                }
            } else {
                assert!(elements.is_empty());
            }
        } else {
            panic!("CONFIG GET should return array");
        }
    };

    send_array(
        &mut writer,
        vec![bulk(b"CONFIG"), bulk(b"GET"), bulk(b"server.name")],
    )?;
    let response = read_frame(&mut reader)?;
    check(response, Some("server.name"));

    send_array(
        &mut writer,
        vec![bulk(b"CONFIG"), bulk(b"GET"), bulk(b"server.unknown")],
    )?;
    let response = read_frame(&mut reader)?;
    check(response, None);

    send_array(&mut writer, vec![bulk(b"HELLO"), bulk(b"3")])?;
    let hello_frame = read_frame(&mut reader)?;
    assert_hello3_map(hello_frame);

    send_array(
        &mut writer,
        vec![bulk(b"CONFIG"), bulk(b"GET"), bulk(b"server.name")],
    )?;
    let response = read_frame(&mut reader)?;
    check(response, Some("server.name"));

    send_array(
        &mut writer,
        vec![bulk(b"CONFIG"), bulk(b"GET"), bulk(b"server.unknown")],
    )?;
    let response = read_frame(&mut reader)?;
    check(response, None);

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
        Server::handle_connection(stream, server_storage, None)?;
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

#[test]
fn benchmark_profile_records_preflight_failure_context() -> Result<()> {
    if !command_exists("redis-cli") || !command_exists("redis-benchmark") {
        eprintln!(
            "skipping benchmark metadata integration test because redis-cli/redis-benchmark are unavailable"
        );
        return Ok(());
    }

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let label = format!("metadata-preflight-{nanos}");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script_path = repo_root.join("scripts/benchmark_profile.sh");

    let status = Command::new("bash")
        .arg(&script_path)
        .arg(&label)
        .current_dir(&repo_root)
        .env("HOST", "127.0.0.1")
        .env("PORT", "1")
        .env("REQUESTS", "1")
        .env("REPEATS", "1")
        .env("MIXES", "1:1")
        .env("MODES", "basic")
        .env("BENCH_TIMEOUT_SECONDS", "0")
        .status()?;

    assert!(
        !status.success(),
        "benchmark profile should fail preflight connectivity when target endpoint is unreachable"
    );

    let profile_dir = find_profile_dir_for_label(&label)?;
    let metadata_path = profile_dir.join("run-metadata.txt");
    let metadata_contents = fs::read_to_string(&metadata_path)?;
    let metadata = parse_metadata(&metadata_contents);

    assert_eq!(
        metadata.get("script_stage").map(String::as_str),
        Some("preflight:connectivity")
    );
    assert_eq!(
        metadata.get("script_exit_kind").map(String::as_str),
        Some("failure")
    );
    assert_eq!(
        metadata.get("run_completion_state").map(String::as_str),
        Some("incomplete")
    );
    assert!(
        metadata
            .get("script_exit_status")
            .is_some_and(|value| value != "0"),
        "script_exit_status should be non-zero for preflight failures"
    );
    assert_eq!(
        metadata.get("last_run_started_index").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        metadata.get("last_run_started_label").map(String::as_str),
        Some("none")
    );
    assert_eq!(
        metadata
            .get("last_run_started_output_file")
            .map(String::as_str),
        Some("none")
    );
    assert_eq!(
        metadata.get("last_run_completed_index").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        metadata.get("last_run_completed_label").map(String::as_str),
        Some("none")
    );
    assert_eq!(
        metadata
            .get("last_run_completed_output_file")
            .map(String::as_str),
        Some("none")
    );
    assert_ne!(
        metadata.get("failure_context").map(String::as_str),
        Some("none"),
        "failure_context should preserve preflight failure details"
    );
    assert!(
        metadata
            .get("failure_context")
            .is_some_and(|value| value.starts_with("preflight:connectivity:")),
        "failure_context should identify a connectivity preflight failure"
    );

    fs::remove_dir_all(profile_dir)?;
    Ok(())
}

#[test]
fn benchmark_profile_records_completion_metadata_for_single_mix_run() -> Result<()> {
    if !command_exists("redis-cli") || !command_exists("redis-benchmark") {
        eprintln!(
            "skipping benchmark metadata integration test because redis-cli/redis-benchmark are unavailable"
        );
        return Ok(());
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let storage = Storage::new();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_signal = Arc::clone(&stop);
    let server_storage = storage.clone();

    let handle = thread::spawn(move || -> Result<()> {
        while !stop_signal.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(error) =
                        Server::handle_connection(stream, server_storage.clone(), None)
                    {
                        if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
                            match io_error.kind() {
                                ErrorKind::ConnectionReset
                                | ErrorKind::BrokenPipe
                                | ErrorKind::UnexpectedEof => {}
                                _ => return Err(error),
                            }
                        } else {
                            return Err(error);
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    });

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let label = format!("metadata-complete-{nanos}");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script_path = repo_root.join("scripts/benchmark_profile.sh");

    let status = Command::new("bash")
        .arg(&script_path)
        .arg(&label)
        .current_dir(&repo_root)
        .env("HOST", addr.ip().to_string())
        .env("PORT", addr.port().to_string())
        .env("REQUESTS", "1")
        .env("REPEATS", "1")
        .env("MIXES", "1:1")
        .env("MODES", "basic")
        .env("BENCH_TIMEOUT_SECONDS", "0")
        .status()?;

    stop.store(true, Ordering::Relaxed);
    handle.join().expect("server thread panicked")?;

    assert!(
        status.success(),
        "benchmark profile should complete successfully for a reachable endpoint"
    );

    let profile_dir = find_profile_dir_for_label(&label)?;
    let metadata_path = profile_dir.join("run-metadata.txt");
    let metadata_contents = fs::read_to_string(&metadata_path)?;
    let metadata = parse_metadata(&metadata_contents);

    assert_eq!(
        metadata.get("total_runs_expected").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        metadata.get("total_runs_completed").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        metadata.get("total_runs_remaining").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        metadata.get("run_completion_state").map(String::as_str),
        Some("complete")
    );
    assert_eq!(
        metadata.get("script_exit_kind").map(String::as_str),
        Some("success")
    );
    assert_eq!(
        metadata.get("script_exit_status").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        metadata.get("script_stage").map(String::as_str),
        Some("complete")
    );
    assert_eq!(
        metadata.get("timeout_probe_exit_final").map(String::as_str),
        Some("disabled")
    );
    assert_eq!(
        metadata.get("last_run_started_index").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        metadata.get("last_run_completed_index").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        metadata.get("last_run_started_label").map(String::as_str),
        Some("resp3:basic:c1:p1:r1")
    );
    assert_eq!(
        metadata.get("last_run_completed_label").map(String::as_str),
        Some("resp3:basic:c1:p1:r1")
    );
    assert!(
        metadata
            .get("last_run_started_output_file")
            .is_some_and(|value| value.ends_with("/resp3-basic-c1-p1-r1.txt")),
        "last_run_started_output_file should point to the final run output"
    );
    assert!(
        metadata
            .get("last_run_completed_output_file")
            .is_some_and(|value| value.ends_with("/resp3-basic-c1-p1-r1.txt")),
        "last_run_completed_output_file should point to the final run output"
    );
    assert_eq!(
        metadata.get("failure_context").map(String::as_str),
        Some("none")
    );

    fs::remove_dir_all(profile_dir)?;
    Ok(())
}
