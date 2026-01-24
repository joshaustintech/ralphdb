use std::{
    io::{self, BufReader, BufWriter, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
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

        Self {
            host,
            port,
            threads: threads.max(1),
        }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
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
        log::info!(
            "ralphdb listening on {} (threads={})",
            self.config.address(),
            self.config.threads
        );

        let pool = ThreadPool::new(self.config.threads);

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
                if let Err(err) = Self::handle_connection(stream, storage) {
                    log::debug!("Connection ended with error: {err}");
                }
            });
        }

        Ok(())
    }

    fn handle_connection(stream: TcpStream, storage: Arc<Storage>) -> anyhow::Result<()> {
        let peer = stream
            .peer_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_default();
        let reader = BufReader::new(stream.try_clone()?);
        let mut writer = BufWriter::new(stream);

        let mut state = command::ConnectionState::default();
        let mut reader = reader;

        loop {
            let frame = match protocol::decode_frame(&mut reader) {
                Ok(frame) => frame,
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
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

            let command = match Command::try_from(frame) {
                Ok(cmd) => cmd,
                Err(err_msg) => {
                    let response = protocol::Frame::Error(err_msg);
                    protocol::encode_frame(&response, state.protocol, &mut writer)?;
                    writer.flush()?;
                    continue;
                }
            };

            let result = command::execute(&command, &storage, &mut state);
            protocol::encode_frame(&result.response, state.protocol, &mut writer)?;
            writer.flush()?;

            if result.close {
                log::info!("{} commanded QUIT", peer);
                break;
            }
        }

        Ok(())
    }
}
