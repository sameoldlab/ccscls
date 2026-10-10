use lsp_server::Connection;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    // NOTE: stdout carries the protocol. Anything diagnostic goes to stderr.
    let (connection, io_threads) = Connection::stdio();
    css_class_lsp::server::run(connection)?;
    io_threads.join()?;
    Ok(())
}
