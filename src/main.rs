mod command;
mod protocol;
mod server;
mod storage;

use std::process;

use env_logger::Env;

fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    if let Err(err) = run() {
        eprintln!("ralphdb failed: {err}");
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let config = server::Config::from_env();
    let storage = storage::Storage::new();

    let server = server::Server::new(config, storage);
    server.run()
}
