pub mod command;
pub mod protocol;
pub mod server;
pub mod storage;

pub fn run() -> anyhow::Result<()> {
    let config = server::Config::from_env();
    let storage = storage::Storage::new();

    let server = server::Server::new(config, storage);
    server.run()
}
