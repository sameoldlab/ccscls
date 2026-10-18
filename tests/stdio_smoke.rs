//! Level 3: the real binary over stdio with Content-Length framing. One test,
//! only there to prove the process wiring (main.rs, framing, clean exit).

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn send(w: &mut impl Write, v: &Value) {
    let body = v.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    w.flush().unwrap();
}

fn recv(r: &mut impl BufRead) -> Value {
    let mut len = None;
    loop {
        let mut line = String::new();
        assert!(r.read_line(&mut line).unwrap() > 0, "server closed stdout");
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length: ") {
            len = Some(v.parse::<usize>().unwrap());
        }
    }
    let mut buf = vec![0; len.expect("no Content-Length header")];
    r.read_exact(&mut buf).unwrap();
    serde_json::from_slice(&buf).unwrap()
}

#[test]
fn binary_speaks_lsp_over_stdio_and_exits_cleanly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_css-class-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();

    #[cfg(unix)]
    {
        // Watchdog: a hung server must fail the test, not hang CI.
        let pid = child.id().to_string();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let _ = Command::new("kill").args(["-9", &pid]).status();
        });
    }

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
    );
    let init = recv(&mut stdout);
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(init["result"]["serverInfo"]["name"], "css-class-lsp");

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    );
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{
        "uri":"file:///virtual/a.css","languageId":"css","version":1,"text":".btn { color: red }"}}}),
    );
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{
        "textDocument":{"uri":"file:///virtual/a.css"},"position":{"line":0,"character":2}}}),
    );
    let hover = recv(&mut stdout);
    assert_eq!(hover["id"], 2);
    let text = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(text.starts_with("**`.btn`** — 1 rule"), "{text}");

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
    );
    let down = recv(&mut stdout);
    assert_eq!(
        (down["id"].as_i64(), down["result"].is_null()),
        (Some(3), true)
    );
    send(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(child.wait().unwrap().success());
}
