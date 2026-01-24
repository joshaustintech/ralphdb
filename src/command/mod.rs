use std::time::Duration;

use crate::{
    protocol::{Frame, ProtocolVersion},
    storage::{Storage, StorageError},
};

pub struct Command {
    pub name: String,
    pub args: Vec<Vec<u8>>,
}

impl TryFrom<Frame> for Command {
    type Error = String;

    fn try_from(value: Frame) -> Result<Self, Self::Error> {
        if let Frame::Array(Some(elements)) = value {
            if elements.is_empty() {
                return Err("ERR missing command".into());
            }

            let mut iter = elements.into_iter();
            let command = iter.next().unwrap();
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
                .map(|frame| match frame {
                    Frame::BulkString(Some(bytes)) => Ok(bytes),
                    Frame::BulkString(None) => Err("ERR null bulk string not allowed".to_string()),
                    Frame::SimpleString(s) => Ok(s.into_bytes()),
                    Frame::Integer(i) => Ok(i.to_string().into_bytes()),
                    _ => Err("ERR unsupported argument type".to_string()),
                })
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

pub struct ConnectionState {
    pub protocol: ProtocolVersion,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            protocol: ProtocolVersion::Resp2,
        }
    }
}

pub struct CommandResult {
    pub response: Frame,
    pub close: bool,
}

impl CommandResult {
    fn ok(response: Frame) -> Self {
        Self {
            response,
            close: false,
        }
    }

    fn error(message: &str) -> Self {
        Self {
            response: Frame::Error(message.to_string()),
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
        "QUIT" => CommandResult {
            response: Frame::SimpleString("OK".into()),
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
    let version = if let Some(version_value) = args.get(0) {
        std::str::from_utf8(version_value)
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(2)
    } else {
        2
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
        Err(StorageError::InvalidInteger) => CommandResult::error("ERR value is not an integer"),
    }
}

fn decr(args: &[Vec<u8>], storage: &Storage) -> CommandResult {
    if args.len() != 1 {
        return CommandResult::error("ERR wrong number of arguments for 'decr'");
    }

    match storage.decr(&args[0]) {
        Ok(value) => CommandResult::ok(Frame::Integer(value)),
        Err(StorageError::InvalidInteger) => CommandResult::error("ERR value is not an integer"),
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

    if args.len() % 2 != 0 {
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
        .get(0)
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
            close: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Frame, ProtocolVersion};
    use std::thread::sleep;
    use std::time::Duration;

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
        sleep(Duration::from_millis(20));

        let mut state = ConnectionState::default();
        let expire_cmd = Command {
            name: "EXPIRE".into(),
            args: vec![b"temp".to_vec(), b"10".to_vec()],
        };
        let result = execute(&expire_cmd, &storage, &mut state);
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
}
