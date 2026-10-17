use css_class_lsp::error::Context;
use lsp_server::Connection;
use std::process::ExitCode;

fn main() -> ExitCode {
    // NOTE: stdout carries the protocol. Anything diagnostic goes to stderr.
    let (connection, io_threads) = Connection::stdio();
    if let Err(error) = css_class_lsp::server::run(connection) {
        // Do not join the io threads here: the reader may still be blocked on stdin.
        eprintln!("css-class-lsp: {error}");
        return ExitCode::FAILURE;
    }
    match io_threads.join().context("stopping the stdio threads") {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("css-class-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
