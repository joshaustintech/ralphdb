use std::{
    io::{self, BufReader, BufWriter, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};

use threadpool::ThreadPool;

use crate::{
    command::{self, Command},
    protocol,
    storage::Storage,
};

#[derive(Clone)]
pub struct Config {
    host: String,
    port: u16,
    threads: usize,
    idle_timeout: Option<Duration>,
}

impl Config {
    pub fn from_env() -> Self {
        let host = std::env::var("RALPHDB_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let port = std::env::var("RALPHDB_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(6379);

        let threads = std::env::var("RALPHDB_THREADS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1)
            });

        const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
        let idle_timeout = match std::env::var("RALPHDB_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
        {
            Some(0) => None,
            Some(secs) if secs > 0 => Some(Duration::from_secs(secs as u64)),
            _ => Some(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS)),
        };

        Self {
            host,
            port,
            threads: threads.max(1),
            idle_timeout,
        }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, time::Duration};

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

    #[test]
    fn zero_idle_timeout_disables_timer() {
        let _guard = EnvVarGuard::set("RALPHDB_IDLE_TIMEOUT_SECS", Some("0"));
        let config = Config::from_env();
        assert!(config.idle_timeout().is_none());
    }

    #[test]
    fn default_idle_timeout_applied_when_missing() {
        let _guard = EnvVarGuard::set("RALPHDB_IDLE_TIMEOUT_SECS", None);
        let config = Config::from_env();
        assert_eq!(config.idle_timeout(), Some(Duration::from_secs(300)));
    }
}

pub struct Server {
    config: Config,
    storage: Arc<Storage>,
}

impl Server {
    pub fn new(config: Config, storage: Arc<Storage>) -> Self {
        Self { config, storage }
    }

    pub fn run(&self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.config.address())?;
        self.serve(listener)
    }

    pub fn serve(&self, listener: TcpListener) -> anyhow::Result<()> {
        let address = listener.local_addr()?.to_string();
        log::info!(
            "ralphdb listening on {} (threads={})",
            address,
            self.config.threads
        );

        let pool = ThreadPool::new(self.config.threads);
        let idle_timeout = self.config.idle_timeout();

        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    log::warn!("Failed to accept connection: {err}");
                    continue;
                }
            };

            let storage = Arc::clone(&self.storage);
            pool.execute(move || {
                if let Err(err) = Self::handle_connection(stream, storage, idle_timeout) {
                    log::debug!("Connection ended with error: {err}");
                }
            });
        }

        Ok(())
    }

    pub fn handle_connection(
        stream: TcpStream,
        storage: Arc<Storage>,
        idle_timeout: Option<Duration>,
    ) -> anyhow::Result<()> {
        let peer = stream
            .peer_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_default();
        let reader_stream = stream.try_clone()?;

        if let Some(timeout) = idle_timeout {
            reader_stream.set_read_timeout(Some(timeout))?;
        }

        let writer_stream = stream;
        if let Some(timeout) = idle_timeout {
            writer_stream.set_read_timeout(Some(timeout))?;
            writer_stream.set_write_timeout(Some(timeout))?;
        }

        let mut reader = BufReader::new(reader_stream);
        let mut writer = BufWriter::new(writer_stream);

        let mut state = command::ConnectionState::default();

        loop {
            let frame = match protocol::decode_frame(&mut reader) {
                Ok(frame) => frame,
                Err(err)
                    if err.kind() == io::ErrorKind::UnexpectedEof
                        || err.kind() == io::ErrorKind::WouldBlock
                        || err.kind() == io::ErrorKind::TimedOut =>
                {
                    log::info!("Closing idle connection for {peer}");
                    break;
                }
                Err(err) => {
                    log::debug!("Malformed frame from {peer}: {err}");
                    let _ = protocol::encode_frame(
                        &protocol::Frame::Error("ERR malformed frame".to_string()),
                        state.protocol,
                        &mut writer,
                    );
                    writer.flush()?;
                    continue;
                }
            };

            let command = match Command::from_frame_with_protocol(frame, state.protocol) {
                Ok(cmd) => cmd,
                Err(err_msg) => {
                    let response = protocol::Frame::Error(err_msg);
                    protocol::encode_frame(&response, state.protocol, &mut writer)?;
                    writer.flush()?;
                    continue;
                }
            };

            let result = command::execute(&command, &storage, &mut state);
            let command::CommandResult {
                response,
                attributes,
                close,
            } = result;
            protocol::encode_response(
                &response,
                attributes.as_deref(),
                state.protocol,
                &mut writer,
            )?;
            writer.flush()?;

            if close {
                log::info!("{} commanded QUIT", peer);
                break;
            }
        }

        Ok(())
    }
}
