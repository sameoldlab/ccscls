//! Ask the real server a question about a folder, from the command line.
//!
//!   cargo run -q --example query -- <dir> classes
//!   cargo run -q --example query -- <dir> <file> <line>:<col> [hover|definition|references|completion]
//!
//! `line` and `col` are 1-based (col counts characters). The server runs
//! in-process through the same message loop the editor talks to, and the file
//! is opened as a live buffer first. Set CSS_CLASS_LSP_LOG=1 to see its log.

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!(
            "usage: query <dir> classes | query <dir> <file> <line>:<col> [hover|definition|references|completion]"
        );
        std::process::exit(2);
    }
    let root = std::fs::canonicalize(&args[0]).expect("workspace dir");
    let (server_conn, conn) = Connection::memory();
    let server = std::thread::spawn(move || csscls::server::run(server_conn));
    let mut next_id = 0;
    let mut call = |method: &str, params: Value| -> Value {
        next_id += 1;
        let id = RequestId::from(next_id);
        conn.sender
            .send(Message::Request(Request::new(
                id.clone(),
                method.into(),
                params,
            )))
            .unwrap();
        loop {
            match conn.receiver.recv().expect("server hung up") {
                Message::Response(r) if r.id == id => {
                    return r
                        .response_result
                        .unwrap_or_else(|e| json!({ "error": e.message }));
                }
                _ => {}
            }
        }
    };
    let notify = |method: &str, params: Value| {
        conn.sender
            .send(Message::Notification(Notification::new(
                method.into(),
                params,
            )))
            .unwrap();
    };

    let root_uri = csscls::uri::path_to_uri(&root).unwrap();
    call(
        "initialize",
        json!({ "capabilities": {}, "workspaceFolders": [{ "uri": root_uri.as_str(), "name": "q" }] }),
    );
    notify("initialized", json!({}));

    if args[1] == "classes" {
        let uri = "file:///query/virtual.html";
        notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "html", "version": 1, "text": "<p class=\"\">" }}),
        );
        let r = call(
            "textDocument/completion",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 10 } }),
        );
        for item in r.as_array().cloned().unwrap_or_default() {
            println!(
                "{:<28} {}",
                item["label"].as_str().unwrap_or(""),
                item["detail"].as_str().unwrap_or("")
            );
        }
    } else {
        let (file, pos, what) = (
            &args[1],
            args.get(2).expect("line:col"),
            args.get(3).map_or("hover", String::as_str),
        );
        let path = std::fs::canonicalize(file)
            .or_else(|_| std::fs::canonicalize(root.join(file)))
            .expect("file");
        let text = std::fs::read_to_string(&path).unwrap();
        let (l, c) = pos.split_once(':').expect("line:col");
        let (line, col): (usize, usize) = (l.parse().unwrap(), c.parse().unwrap());
        let utf16: usize = text
            .lines()
            .nth(line - 1)
            .expect("no such line")
            .chars()
            .take(col - 1)
            .map(char::len_utf16)
            .sum();
        let uri = csscls::uri::path_to_uri(&path).unwrap();
        let lang = if path.extension().is_some_and(|e| e == "css") {
            "css"
        } else {
            "html"
        };
        notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri.as_str(), "languageId": lang, "version": 1, "text": text }}),
        );
        let position = json!({ "line": line - 1, "character": utf16 });
        let tdp = json!({ "textDocument": { "uri": uri.as_str() }, "position": position });
        match what {
            "hover" => {
                let r = call("textDocument/hover", tdp);
                println!(
                    "{}",
                    r["contents"]["value"]
                        .as_str()
                        .unwrap_or("(nothing under the cursor)")
                );
            }
            "definition" => print_locations(&root, call("textDocument/definition", tdp)),
            "references" => {
                let mut p = tdp;
                p["context"] = json!({ "includeDeclaration": true });
                print_locations(&root, call("textDocument/references", p));
            }
            "completion" => {
                let r = call("textDocument/completion", tdp);
                let items = r.as_array().cloned().unwrap_or_default();
                if items.is_empty() {
                    println!("(not in a class value)");
                }
                for item in items {
                    println!(
                        "{:<28} {}",
                        item["label"].as_str().unwrap_or(""),
                        item["detail"].as_str().unwrap_or("")
                    );
                }
            }
            other => panic!("unknown action {other}"),
        }
    }

    call("shutdown", Value::Null);
    notify("exit", Value::Null);
    server.join().unwrap().unwrap();
}

fn print_locations(root: &Path, r: Value) {
    let locs: Vec<Location> = serde_json::from_value(r).unwrap_or_default();
    if locs.is_empty() {
        println!("(none)");
    }
    for l in locs {
        let path: PathBuf = csscls::uri::uri_to_path(&l.uri).unwrap();
        let shown = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let line = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| {
                t.lines()
                    .nth(l.range.start.line as usize)
                    .map(|s| s.trim().to_owned())
            })
            .unwrap_or_default();
        println!(
            "{shown}:{}:{}  | {line}",
            l.range.start.line + 1,
            l.range.start.character + 1
        );
    }
}
