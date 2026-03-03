use std::env;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use crate::{
    protocol::{Frame, ProtocolVersion},
    storage::{Storage, StorageError},
};
use log::debug;

static CLIENT_ID_COUNTER: AtomicI64 = AtomicI64::new(1);

pub struct Command {
    pub name: String,
    pub args: Vec<Vec<u8>>,
}

impl Command {
    pub fn from_frame_with_protocol(
        value: Frame,
        protocol: ProtocolVersion,
    ) -> Result<Self, String> {
        if let Frame::Array(Some(elements)) = value {
            if elements.is_empty() {
                return Err("ERR missing command".into());
            }

            let mut iter = elements.into_iter();
            let command = iter
                .next()
                .ok_or_else(|| "ERR missing command".to_string())?;
            let name = match command {
                Frame::SimpleString(s) => s,
                Frame::BulkString(Some(bytes)) => String::from_utf8(bytes)
                    .map_err(|_| "ERR invalid command name encoding".to_string())?,
                Frame::BulkString(None) => {
                    return Err("ERR invalid command name".into());
                }
                _ => {
                    return Err("ERR invalid command name".into());
                }
            };

            let args = iter
                .map(|frame| to_arg_bytes(frame, protocol))
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Command {
                name: name.to_ascii_uppercase(),
                args,
            })
        } else {
            Err("ERR command must be an array".into())
        }
    }
}

impl TryFrom<Frame> for Command {
    type Error = String;

    fn try_from(value: Frame) -> Result<Self, Self::Error> {
        Self::from_frame_with_protocol(value, ProtocolVersion::Resp2)
    }
}

fn to_arg_bytes(frame: Frame, protocol: ProtocolVersion) -> Result<Vec<u8>, String> {
    match frame {
        Frame::BulkString(Some(bytes)) => Ok(bytes),
        Frame::BulkString(None) => Err("ERR null bulk string not allowed".to_string()),
        Frame::SimpleString(s) => Ok(s.into_bytes()),
        Frame::Integer(i) => Ok(i.to_string().into_bytes()),
        Frame::Boolean(value) if protocol == ProtocolVersion::Resp3 => Ok(if value {
            b"true".to_vec()
        } else {
            b"false".to_vec()
        }),
        Frame::Double(value) if protocol == ProtocolVersion::Resp3 => {
            Ok(value.to_string().into_bytes())
        }
        Frame::BigNumber(value) if protocol == ProtocolVersion::Resp3 => Ok(value.into_bytes()),
        Frame::VerbatimString { payload, .. } if protocol == ProtocolVersion::Resp3 => Ok(payload),
        _ => Err("ERR unsupported argument type".to_string()),
    }
}

pub struct ConnectionState {
    pub protocol: ProtocolVersion,
    pub client_name: Option<Vec<u8>>,
    pub client_id: i64,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            protocol: ProtocolVersion::Resp2,
            client_name: None,
            client_id: CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }
}

pub struct CommandResult {
    pub response: Frame,
    pub attributes: Option<Vec<(Frame, Frame)>>,
    pub close: bool,
}

impl CommandResult {
    fn ok(response: Frame) -> Self {
        Self {
            response,
            attributes: None,
            close: false,
        }
    }

    fn with_attributes(response: Frame, attributes: Vec<(Frame, Frame)>) -> Self {
        Self {
            response,
            attributes: Some(attributes),
            close: false,
        }
    }

    fn error(message: &str) -> Self {
        Self {
            response: Frame::Error(message.to_string()),
            attributes: None,
            close: false,
        }
    }
}

pub fn execute(command: &Command, storage: &Storage, state: &mut ConnectionState) -> CommandResult {
    match command.name.as_str() {
        "PING" => ping(&command.args),
        "ECHO" => echo(&command.args),
        "HELLO" => hello(&command.args, state),
        "INFO" => info(&command.args),
        "CONFIG" => config(&command.args),
        "CLIENT" => client(&command.args, state),
        "QUIT" => CommandResult {
            response: Frame::SimpleString("OK".into()),
            attributes: None,
            close: true,
        },
        "SET" => set(&command.args, storage),
        "GET" => get(&command.args, storage),
        "DEL" => del(&command.args, storage),
        "EXISTS" => exists(&command.args, storage),
        "INCR" => incr(&command.args, storage),
        "DECR" => decr(&command.args, storage),
        "MGET" => mget(&command.args, storage),
        "MSET" => mset(&command.args, storage),
        "EXPIRE" => expire(&command.args, storage),
        "TTL" => ttl(&command.args, storage),
        _ => CommandResult::error("ERR unknown command"),
    }
}

fn ping(args: &[Vec<u8>]) -> CommandResult {
    match args.len() {
        0 => CommandResult::ok(Frame::SimpleString("PONG".into())),
        1 => CommandResult::ok(Frame::BulkString(Some(args[0].clone()))),
        _ => CommandResult::error("ERR wrong number of arguments for 'ping' command"),
    }
}

fn echo(args: &[Vec<u8>]) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'echo' command");
    }
    CommandResult::ok(Frame::BulkString(Some(args[0].clone())))
}

fn hello(args: &[Vec<u8>], state: &mut ConnectionState) -> CommandResult {
    if args.len() > 1 {
        return CommandResult::error("ERR wrong number of arguments for 'hello' command");
    }

    let version = match args.first() {
        Some(version_value) => match std::str::from_utf8(version_value)
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
        {
            Some(version) => version,
            None => return CommandResult::error("ERR unsupported RESP version"),
        },
        None => 2,
    };

    match version {
        3 => {
            state.protocol = ProtocolVersion::Resp3;
            let payload = Frame::Map(Some(vec![
                (
                    Frame::SimpleString("server".into()),
                    Frame::SimpleString("ralphdb".into()),
                ),
                (
                    Frame::SimpleString("version".into()),
                    Frame::SimpleString(env!("CARGO_PKG_VERSION").into()),
                ),
                (Frame::SimpleString("proto".into()), Frame::Integer(3)),
                (
                    Frame::SimpleString("id".into()),
                    Frame::SimpleString(env!("CARGO_PKG_NAME").into()),
                ),
                (
                    Frame::SimpleString("mode".into()),
                    Frame::SimpleString("standalone".into()),
                ),
                (
                    Frame::SimpleString("role".into()),
                    Frame::SimpleString("primary".into()),
                ),
                (
                    Frame::SimpleString("modules".into()),
                    Frame::Array(Some(vec![])),
                ),
            ]));
            CommandResult::ok(payload)
        }
        2 => {
            state.protocol = ProtocolVersion::Resp2;
            CommandResult::ok(Frame::SimpleString("OK".into()))
        }
        _ => CommandResult::error("ERR unsupported RESP version"),
    }
}

fn set(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() < 2 {
        return CommandResult::error("ERR wrong number of arguments for 'set'");
    }

    let key = args[0].clone();
    let value = args[1].clone();
    storage.set(key, value);
    CommandResult::ok(Frame::SimpleString("OK".into()))
}

fn get(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'get'");
    }

    match storage.get(&args[0]) {
        Some(value) => CommandResult::ok(Frame::BulkString(Some(value))),
        None => CommandResult::ok(Frame::Null),
    }
}

fn del(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'del'");
    }

    let count = storage.del(args);
    CommandResult::ok(Frame::Integer(count as i64))
}

fn exists(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'exists'");
    }

    let count = args.iter().filter(|key| storage.exists(key)).count();
    CommandResult::ok(Frame::Integer(count as i64))
}

fn incr(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'incr'");
    }

    match storage.incr(&args[0]) {
        Ok(value) => CommandResult::ok(Frame::Integer(value)),
        Err(StorageError::InvalidInteger | StorageError::IntegerOutOfRange) => {
            CommandResult::error("ERR value is not an integer or out of range")
        }
    }
}

fn decr(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'decr'");
    }

    match storage.decr(&args[0]) {
        Ok(value) => CommandResult::ok(Frame::Integer(value)),
        Err(StorageError::InvalidInteger | StorageError::IntegerOutOfRange) => {
            CommandResult::error("ERR value is not an integer or out of range")
        }
    }
}

fn mget(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'mget'");
    }

    let values = storage.mget(args);
    let frames = values
        .into_iter()
        .map(|value| match value {
            Some(bytes) => Frame::BulkString(Some(bytes)),
            None => Frame::Null,
        })
        .collect();

    CommandResult::ok(Frame::Array(Some(frames)))
}

fn mset(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'mset'");
    }

    if !args.len().is_multiple_of(2) {
        return CommandResult::error("ERR wrong number of arguments for 'mset'");
    }

    let pairs = args
        .chunks(2)
        .map(|chunk| (chunk[0].clone(), chunk[1].clone()))
        .collect::<Vec<_>>();
    storage.mset(&pairs);
    CommandResult::ok(Frame::SimpleString("OK".into()))
}

fn expire(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() != 2 {
        return CommandResult::error("ERR wrong number of arguments for 'expire'");
    }

    let duration = match std::str::from_utf8(&args[1])
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        Some(secs) => Duration::from_secs(secs),
        None => return CommandResult::error("ERR value is not an integer or out of range"),
    };

    if storage.expire(&args[0], duration) {
        CommandResult::ok(Frame::Integer(1))
    } else {
        CommandResult::ok(Frame::Integer(0))
    }
}

fn ttl(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'ttl'");
    }

    let value = storage.ttl(&args[0]);
    CommandResult::ok(Frame::Integer(value))
}

fn info(args: &[Vec<u8>]) -> CommandResult {
    if args.len() > 1 {
        return CommandResult::error("ERR wrong number of arguments for 'info'");
    }

    let section = args
        .first()
        .map(|bytes| String::from_utf8_lossy(bytes).to_ascii_lowercase())
        .unwrap_or_else(|| "default".into());

    match section.as_str() {
        "default" | "server" => {
            let version = env!("CARGO_PKG_VERSION");
            let server_id = env!("CARGO_PKG_NAME");
            let body = format!(
                "# Server\r\nralphdb_version:{}\r\nralphdb_mode:standalone\r\nralphdb_role:primary\r\nralphdb_id:{}\r\n",
                version, server_id
            );
            CommandResult::ok(Frame::BulkString(Some(body.into_bytes())))
        }
        other => CommandResult {
            response: Frame::Error(format!("ERR unsupported INFO section '{other}'")),
            attributes: None,
            close: false,
        },
    }
}

fn client(args: &[Vec<u8>], state: &mut ConnectionState) -> CommandResult {
    if args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'client' command");
    }

    let subcommand = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();

    match subcommand.as_str() {
        "SETNAME" => client_setname(&args[1..], state),
        "GETNAME" => {
            if args.len() != 1 {
                return CommandResult::error("ERR wrong number of arguments for 'client getname'");
            }
            client_getname(state)
        }
        "LIST" => client_list(&args[1..], state),
        "ID" => client_id(&args[1..], state),
        _ => CommandResult::error("ERR unknown CLIENT subcommand"),
    }
}

fn client_setname(args: &[Vec<u8>], state: &mut ConnectionState) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'client setname'");
    }

    state.client_name = Some(args[0].clone());
    CommandResult::ok(Frame::SimpleString("OK".into()))
}

fn client_getname(state: &ConnectionState) -> CommandResult {
    match &state.client_name {
        Some(name) => CommandResult::ok(Frame::BulkString(Some(name.clone()))),
        None => CommandResult::ok(Frame::Null),
    }
}

fn client_id(args: &[Vec<u8>], state: &ConnectionState) -> CommandResult {
    if !args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'client id'");
    }

    CommandResult::ok(Frame::Integer(state.client_id))
}

fn client_list(args: &[Vec<u8>], state: &ConnectionState) -> CommandResult {
    if !args.is_empty() {
        return CommandResult::error("ERR wrong number of arguments for 'client list'");
    }

    let summary = client_list_summary(state);

    if state.protocol == ProtocolVersion::Resp3 {
        let name_frame = match &state.client_name {
            Some(name) => Frame::BulkString(Some(name.clone())),
            None => Frame::Null,
        };

        let client_map = vec![
            (
                Frame::SimpleString("id".into()),
                Frame::Integer(state.client_id),
            ),
            (Frame::SimpleString("name".into()), name_frame),
            (
                Frame::SimpleString("protocol".into()),
                Frame::SimpleString("RESP3".into()),
            ),
        ];

        let attribute = vec![(
            Frame::SimpleString("client".into()),
            Frame::Push(vec![Frame::Map(Some(client_map))]),
        )];

        CommandResult::with_attributes(Frame::SimpleString(summary), attribute)
    } else {
        CommandResult::ok(Frame::SimpleString(summary))
    }
}

fn client_list_summary(state: &ConnectionState) -> String {
    let mut summary = format!("id={}", state.client_id);
    if let Some(name) = &state.client_name {
        summary.push_str(&format!(" name={}", String::from_utf8_lossy(name)));
    } else {
        summary.push_str(" name=(null)");
    }
    let proto_string = match state.protocol {
        ProtocolVersion::Resp3 => "RESP3",
        ProtocolVersion::Resp2 => "RESP2",
    };
    summary.push_str(&format!(" protocol={proto_string}"));
    summary
}

fn config(args: &[Vec<u8>]) -> CommandResult {
    debug!("CONFIG request args (len={}): {:?}", args.len(), args);
    if args.len() != 2 {
        return CommandResult::error("ERR wrong number of arguments for 'config' command");
    }

    let subcommand = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
    if subcommand != "GET" {
        return CommandResult::error("ERR only CONFIG GET is supported");
    }

    let pattern = match std::str::from_utf8(&args[1]) {
        Ok(value) => value,
        Err(_) => {
            return CommandResult::error("ERR invalid CONFIG GET pattern");
        }
    };

    let entries: Vec<_> = config_entries()
        .into_iter()
        .filter(|(key, _)| matches_pattern(pattern, key))
        .collect();

    let mut frames = Vec::with_capacity(entries.len() * 2);
    for (key, value) in entries {
        frames.push(Frame::BulkString(Some(key.into_bytes())));
        frames.push(Frame::BulkString(Some(value.into_bytes())));
    }

    CommandResult::ok(Frame::Array(Some(frames)))
}

fn config_entries() -> Vec<(String, String)> {
    let host = env::var("RALPHDB_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var("RALPHDB_PORT").unwrap_or_else(|_| "6379".into());
    let threads = default_threads();

    vec![
        ("server.name".into(), "ralphdb".into()),
        ("server.version".into(), env!("CARGO_PKG_VERSION").into()),
        ("server.bind".into(), host),
        ("server.port".into(), port),
        ("server.threads".into(), threads.to_string()),
        ("save".into(), "900 1 300 10 60 10000".into()),
        ("appendonly".into(), "no".into()),
    ]
}

fn default_threads() -> usize {
    env::var("RALPHDB_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
}

fn matches_pattern(pattern: &str, key: &str) -> bool {
    if pattern == "*" {
        true
    } else if pattern.is_empty() {
        false
    } else if pattern.starts_with('*') && pattern.ends_with('*') && pattern.len() > 1 {
        let inner = &pattern[1..pattern.len() - 1];
        key.contains(inner)
    } else if let Some(suffix) = pattern.strip_prefix('*') {
        key.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        key.starts_with(prefix)
    } else {
        key == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Frame, ProtocolVersion};
    use std::{
        thread::sleep,
        time::{Duration, Instant},
    };

    fn wait_for_expiration(storage: &Storage, key: &[u8]) {
        let timeout = Duration::from_secs(1);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if storage.get(key).is_none() {
                return;
            }
            sleep(Duration::from_millis(5));
        }
        panic!(
            "key {:?} did not expire within {:?}",
            String::from_utf8_lossy(key),
            timeout
        );
    }

    #[test]
    fn parse_set_get_command() {
        let frame = Frame::Array(Some(vec![
            Frame::BulkString(Some(b"SET".to_vec())),
            Frame::BulkString(Some(b"key".to_vec())),
            Frame::BulkString(Some(b"value".to_vec())),
        ]));

        let command = Command::try_from(frame).expect("should parse");
        assert_eq!(command.name, "SET");
        assert_eq!(command.args.len(), 2);
    }

    #[test]
    fn reject_invalid_utf8_command_name() {
        let frame = Frame::Array(Some(vec![Frame::BulkString(Some(vec![0x80]))]));
        match Command::try_from(frame) {
            Ok(_) => panic!("invalid command name should error"),
            Err(error) => assert_eq!(error, "ERR invalid command name encoding"),
        }
    }

    #[test]
    fn execute_set_and_get() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();

        let set_cmd = Command {
            name: "SET".into(),
            args: vec![b"counter".to_vec(), b"5".to_vec()],
        };
        let _ = execute(&set_cmd, &storage, &mut state);

        let get_cmd = Command {
            name: "GET".into(),
            args: vec![b"counter".to_vec()],
        };
        let result = execute(&get_cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::BulkString(Some(ref value)) if value == b"5"));
    }

    #[test]
    fn hello_sets_resp3() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        assert_eq!(state.protocol, ProtocolVersion::Resp2);

        let hello_cmd = Command {
            name: "HELLO".into(),
            args: vec![b"3".to_vec()],
        };
        let result = execute(&hello_cmd, &storage, &mut state);
        assert_eq!(state.protocol, ProtocolVersion::Resp3);
        if let Frame::Map(Some(entries)) = result.response {
            let mut has_server = false;
            let mut has_version = false;
            let mut has_proto = false;
            let mut has_id = false;
            let mut has_mode = false;
            let mut has_role = false;
            let mut has_modules = false;
            for (key, value) in entries {
                match (key, value) {
                    (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "server" => {
                        assert_eq!(value, "ralphdb");
                        has_server = true;
                    }
                    (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "version" => {
                        assert_eq!(value, env!("CARGO_PKG_VERSION"));
                        has_version = true;
                    }
                    (Frame::SimpleString(key), Frame::Integer(value)) if key == "proto" => {
                        assert_eq!(value, 3);
                        has_proto = true;
                    }
                    (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "id" => {
                        assert_eq!(value, env!("CARGO_PKG_NAME"));
                        has_id = true;
                    }
                    (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "mode" => {
                        assert_eq!(value, "standalone");
                        has_mode = true;
                    }
                    (Frame::SimpleString(key), Frame::SimpleString(value)) if key == "role" => {
                        assert_eq!(value, "primary");
                        has_role = true;
                    }
                    (Frame::SimpleString(key), Frame::Array(Some(elements)))
                        if key == "modules" =>
                    {
                        assert!(elements.is_empty());
                        has_modules = true;
                    }
                    _ => continue,
                }
            }
            assert!(
                has_server
                    && has_version
                    && has_proto
                    && has_id
                    && has_mode
                    && has_role
                    && has_modules
            );
        } else {
            panic!("HELLO 3 response should be a map");
        }
    }

    #[test]
    fn hello_defaults_to_resp2() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let hello_cmd = Command {
            name: "HELLO".into(),
            args: vec![],
        };
        let result = execute(&hello_cmd, &storage, &mut state);
        assert_eq!(state.protocol, ProtocolVersion::Resp2);
        assert!(matches!(result.response, Frame::SimpleString(ref value) if value == "OK"));
    }

    #[test]
    fn hello_version_two_stays_resp2() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let hello_cmd = Command {
            name: "HELLO".into(),
            args: vec![b"2".to_vec()],
        };
        let result = execute(&hello_cmd, &storage, &mut state);
        assert_eq!(state.protocol, ProtocolVersion::Resp2);
        assert!(matches!(result.response, Frame::SimpleString(ref value) if value == "OK"));
    }

    #[test]
    fn hello_rejects_extra_arguments() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let hello_cmd = Command {
            name: "HELLO".into(),
            args: vec![b"3".to_vec(), b"ignored".to_vec()],
        };
        let result = execute(&hello_cmd, &storage, &mut state);

        assert!(matches!(
            result.response,
            Frame::Error(ref value) if value == "ERR wrong number of arguments for 'hello' command"
        ));
        assert_eq!(state.protocol, ProtocolVersion::Resp2);
    }

    #[test]
    fn hello_rejects_nonnumeric_version() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let hello_cmd = Command {
            name: "HELLO".into(),
            args: vec![b"three".to_vec()],
        };
        let result = execute(&hello_cmd, &storage, &mut state);
        assert!(matches!(
            result.response,
            Frame::Error(ref value) if value == "ERR unsupported RESP version"
        ));
        assert_eq!(state.protocol, ProtocolVersion::Resp2);
    }

    #[test]
    fn hello_rejects_non_utf8_version() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let hello_cmd = Command {
            name: "HELLO".into(),
            args: vec![vec![0xff]],
        };
        let result = execute(&hello_cmd, &storage, &mut state);
        assert!(matches!(
            result.response,
            Frame::Error(ref value) if value == "ERR unsupported RESP version"
        ));
        assert_eq!(state.protocol, ProtocolVersion::Resp2);
    }

    #[test]
    fn ping_errors_with_extra_args() {
        let args = vec![b"one".to_vec(), b"two".to_vec()];
        let result = ping(&args);
        assert!(matches!(result.response, Frame::Error(_)));
    }

    #[test]
    fn reject_null_bulk_argument() {
        let frame = Frame::Array(Some(vec![
            Frame::BulkString(Some(b"PING".to_vec())),
            Frame::BulkString(None),
        ]));
        assert!(Command::try_from(frame).is_err());
    }

    #[test]
    fn mset_requires_arguments() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let cmd = Command {
            name: "MSET".into(),
            args: vec![],
        };
        let result = execute(&cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::Error(_)));
    }

    #[test]
    fn expire_on_expired_key_returns_zero() {
        let storage = Storage::new();
        storage.set(b"temp".to_vec(), b"value".to_vec());
        assert!(storage.expire(b"temp", Duration::from_millis(5)));
        wait_for_expiration(&storage, b"temp");

        let mut state = ConnectionState::default();
        let expire_cmd = Command {
            name: "EXPIRE".into(),
            args: vec![b"temp".to_vec(), b"10".to_vec()],
        };
        let result = execute(&expire_cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::Integer(0)));
    }

    #[test]
    fn del_on_unreaped_expired_key_returns_zero() {
        let storage = Storage::new();
        storage.set(b"temp".to_vec(), b"value".to_vec());
        assert!(storage.expire(b"temp", Duration::from_millis(5)));
        sleep(Duration::from_millis(30));

        let mut state = ConnectionState::default();
        let del_cmd = Command {
            name: "DEL".into(),
            args: vec![b"temp".to_vec()],
        };
        let result = execute(&del_cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::Integer(0)));
    }

    #[test]
    fn info_returns_server_metadata() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let info_cmd = Command {
            name: "INFO".into(),
            args: vec![],
        };
        let result = execute(&info_cmd, &storage, &mut state);
        if let Frame::BulkString(Some(bytes)) = result.response {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("ralphdb_version"));
            assert!(text.contains(env!("CARGO_PKG_VERSION")));
        } else {
            panic!("INFO should return a bulk string");
        }
    }

    #[test]
    fn info_rejects_extra_arguments() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let info_cmd = Command {
            name: "INFO".into(),
            args: vec![b"server".to_vec(), b"extra".to_vec()],
        };
        let result = execute(&info_cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::Error(_)));
    }

    #[test]
    fn info_rejects_unknown_section() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let info_cmd = Command {
            name: "INFO".into(),
            args: vec![b"unknown".to_vec()],
        };
        let result = execute(&info_cmd, &storage, &mut state);
        assert!(
            matches!(result.response, Frame::Error(ref message) if message.contains("unsupported INFO section"))
        );
    }

    #[test]
    fn client_setname_updates_state() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let cmd = Command {
            name: "CLIENT".into(),
            args: vec![b"SETNAME".to_vec(), b"ralph".to_vec()],
        };

        let result = execute(&cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::SimpleString(ref value) if value == "OK"));
        assert_eq!(state.client_name.unwrap(), b"ralph".to_vec());
    }

    #[test]
    fn client_setname_rejects_wrong_arity() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let cmd = Command {
            name: "CLIENT".into(),
            args: vec![b"SETNAME".to_vec()],
        };
        let result = execute(&cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::Error(_)));
    }

    #[test]
    fn client_unknown_subcommand_errors() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let cmd = Command {
            name: "CLIENT".into(),
            args: vec![b"NOPE".to_vec()],
        };
        let result = execute(&cmd, &storage, &mut state);
        assert!(
            matches!(result.response, Frame::Error(ref message) if message.contains("unknown CLIENT subcommand"))
        );
    }

    #[test]
    fn client_getname_returns_null_if_not_set() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let cmd = Command {
            name: "CLIENT".into(),
            args: vec![b"GETNAME".to_vec()],
        };
        let result = execute(&cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::Null));
    }

    #[test]
    fn client_getname_returns_value_after_setname() {
        let storage = Storage::new();
        let mut state = ConnectionState::default();
        let set_cmd = Command {
            name: "CLIENT".into(),
            args: vec![b"SETNAME".to_vec(), b"ralph".to_vec()],
        };
        let _ = execute(&set_cmd, &storage, &mut state);

        let get_cmd = Command {
            name: "CLIENT".into(),
            args: vec![b"GETNAME".to_vec()],
        };
        let result = execute(&get_cmd, &storage, &mut state);
        assert!(matches!(result.response, Frame::BulkString(Some(ref value)) if value == b"ralph"));
    }

    #[test]
    fn resp3_double_exponent_coerces_to_string() {
        let value = Frame::Array(Some(vec![
            Frame::BulkString(Some(b"PING".to_vec())),
            Frame::Double(3.1e-5),
        ]));
        let command =
            Command::from_frame_with_protocol(value, ProtocolVersion::Resp3).expect("should parse");
        assert_eq!(command.args.len(), 1);
        assert_eq!(command.args[0], 3.1e-5f64.to_string().into_bytes());
    }

    #[test]
    fn resp3_double_special_values_coerced_to_bytes() {
        let special_values = [f64::INFINITY, f64::NEG_INFINITY, f64::NAN];
        for &value in &special_values {
            let frame = Frame::Array(Some(vec![
                Frame::BulkString(Some(b"PING".to_vec())),
                Frame::Double(value),
            ]));
            let command = Command::from_frame_with_protocol(frame, ProtocolVersion::Resp3)
                .expect("should parse");
            assert_eq!(command.args.len(), 1);
            assert_eq!(command.args[0], value.to_string().into_bytes());
        }
    }

    #[test]
    fn resp3_scalar_arguments_coerced_to_bytes() {
        let value = Frame::Array(Some(vec![
            Frame::BulkString(Some(b"PING".to_vec())),
            Frame::Boolean(true),
            Frame::Double(2.5),
            Frame::BigNumber("12345678901234567890".into()),
            Frame::VerbatimString {
                format: "txt".into(),
                payload: b"payload".to_vec(),
            },
        ]));

        let command =
            Command::from_frame_with_protocol(value, ProtocolVersion::Resp3).expect("should parse");

        assert_eq!(command.args.len(), 4);
        assert_eq!(command.args[0], b"true".to_vec());
        assert_eq!(command.args[1], b"2.5".to_vec());
        assert_eq!(command.args[2], b"12345678901234567890".to_vec());
        assert_eq!(command.args[3], b"payload".to_vec());
    }

    #[test]
    fn resp2_scalar_arguments_still_rejected() {
        let frame = Frame::Array(Some(vec![
            Frame::BulkString(Some(b"PING".to_vec())),
            Frame::Boolean(false),
        ]));

        assert!(Command::from_frame_with_protocol(frame, ProtocolVersion::Resp2).is_err());
    }

    #[test]
    fn config_get_all_keys_exposed() {
        let result = config(&[b"GET".to_vec(), b"*".to_vec()]);
        if let Frame::Array(Some(elements)) = result.response {
            assert!(elements.len() % 2 == 0);
            let keys: Vec<_> = elements
                .chunks(2)
                .filter_map(|pair| {
                    if let [Frame::BulkString(Some(key)), _] = pair {
                        Some(String::from_utf8_lossy(key).to_string())
                    } else {
                        None
                    }
                })
                .collect();
            assert!(keys.contains(&"server.name".into()));
            assert!(keys.contains(&"server.version".into()));
            assert!(keys.contains(&"server.port".into()));
        } else {
            panic!("CONFIG GET * should return an array");
        }
    }

    #[test]
    fn config_get_prefix_pattern_matches() {
        let result = config(&[b"GET".to_vec(), b"server.*".to_vec()]);
        if let Frame::Array(Some(elements)) = result.response {
            assert!(elements.len() % 2 == 0);
            assert!(elements.len() >= 2);
        } else {
            panic!("CONFIG GET server.* should return an array");
        }
    }

    #[test]
    fn client_id_returns_connection_identifier() {
        let state = ConnectionState::default();
        let result = client_id(&[], &state);
        assert!(matches!(result.response, Frame::Integer(value) if value == state.client_id));
    }

    #[test]
    fn client_list_resp3_uses_attributes_and_push() {
        let state = ConnectionState {
            protocol: ProtocolVersion::Resp3,
            client_name: Some(b"ralph".to_vec()),
            ..ConnectionState::default()
        };
        let client_id_value = state.client_id;

        let result = client_list(&[], &state);
        assert!(
            matches!(result.response, Frame::SimpleString(ref summary) if summary.contains("protocol=RESP3"))
        );
        let attributes = result
            .attributes
            .expect("RESP3 CLIENT LIST should include attributes");
        assert_eq!(attributes.len(), 1);
        let (key, value) = &attributes[0];
        assert!(matches!(key, Frame::SimpleString(name) if name == "client"));
        if let Frame::Push(elements) = value {
            assert_eq!(elements.len(), 1);
            if let Frame::Map(Some(entries)) = &elements[0] {
                let mut saw_id = false;
                let mut saw_name = false;
                for (entry_key, entry_value) in entries {
                    if let Frame::SimpleString(entry_key) = entry_key {
                        match (entry_key.as_str(), entry_value) {
                            ("id", Frame::Integer(value)) if *value == client_id_value => {
                                saw_id = true;
                            }
                            ("name", Frame::BulkString(Some(bytes))) if bytes == b"ralph" => {
                                saw_name = true;
                            }
                            _ => {}
                        }
                    }
                }
                assert!(saw_id);
                assert!(saw_name);
            } else {
                panic!("CLIENT LIST push element should be a map");
            }
        } else {
            panic!("CLIENT LIST attribute should hold a push frame");
        }
    }

    #[test]
    fn client_list_resp2_returns_summary_string() {
        let state = ConnectionState {
            protocol: ProtocolVersion::Resp2,
            client_name: None,
            ..ConnectionState::default()
        };

        let result = client_list(&[], &state);
        assert!(result.attributes.is_none());
        if let Frame::SimpleString(text) = result.response {
            assert!(text.contains("id="));
            assert!(text.contains("name=(null)"));
            assert!(text.contains("protocol=RESP2"));
        } else {
            panic!("CLIENT LIST RESP2 should report a simple string");
        }
    }

    #[test]
    fn matches_pattern_suffix_wildcard() {
        assert!(matches_pattern("*name", "client.name"));
        assert!(!matches_pattern("*name", "client.id"));
    }

    #[test]
    fn matches_pattern_prefix_wildcard() {
        assert!(matches_pattern("server.*", "server.port"));
        assert!(!matches_pattern("server.*", "client.name"));
    }

    #[test]
    fn matches_pattern_inner_wildcard() {
        assert!(matches_pattern("*inner*", "begin_inner_end"));
        assert!(!matches_pattern("*inner*", "prefixpost"));
    }

    #[test]
    fn matches_pattern_empty() {
        assert!(!matches_pattern("", "anything"));
    }

    #[test]
    fn incr_reports_error_when_integer_would_overflow() {
        let storage = Storage::new();
        storage.set(b"counter".to_vec(), i64::MAX.to_string().into_bytes());
        let mut state = ConnectionState::default();
        let cmd = Command {
            name: "INCR".into(),
            args: vec![b"counter".to_vec()],
        };
        let result = execute(&cmd, &storage, &mut state);
        assert!(
            matches!(result.response, Frame::Error(ref message) if message.contains("out of range"))
        );
    }
}
