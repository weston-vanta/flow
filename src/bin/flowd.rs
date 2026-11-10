//! `flowd`: the daemon on the Flow CDE, run in the foreground. It serves the read API
//! (`GET /api/sessions`, `GET /api/prs`, the same JSON as `flow session list` and
//! `flow pr list`) and the embedded frontend, on `127.0.0.1` only. Every request reads the
//! Store fresh.

use std::io::Cursor;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;
use tiny_http::{Header, Method, Request, Response, Server};

use flow::services;
use flow::store::Store;

/// Serve flow's read API and web app on 127.0.0.1.
#[derive(Parser)]
#[command(name = "flowd")]
struct Args {
    /// The loopback port to listen on; 0 picks a free one.
    #[arg(long, env = "FLOWD_PORT", default_value_t = 7171)]
    port: u16,
}

/// The frontend bundle (`web/dist`), embedded at build time: (path, content type, body).
const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/",
        "text/html; charset=utf-8",
        include_bytes!("../../web/dist/index.html"),
    ),
    (
        "/app.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../../web/dist/app.js"),
    ),
    (
        "/app.css",
        "text/css; charset=utf-8",
        include_bytes!("../../web/dist/app.css"),
    ),
];

fn main() -> ExitCode {
    let args = Args::parse();
    let store = Store::from_env();
    let server = match Server::http(("127.0.0.1", args.port)) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("flowd: cannot listen on 127.0.0.1:{}: {err}", args.port);
            return ExitCode::FAILURE;
        }
    };
    let addr = server
        .server_addr()
        .to_ip()
        .expect("bound to an IP address");
    println!("flowd listening on http://{addr}");
    // The PR Writer's poll loop and the retention sweep start here, each as a `std::thread`
    // over a clone of `store`, before serving takes over this thread.
    for request in server.incoming_requests() {
        let response = respond(&store, &request);
        if let Err(err) = request.respond(response) {
            eprintln!("flowd: cannot send a response: {err}");
        }
    }
    ExitCode::SUCCESS
}

fn respond(store: &Store, request: &Request) -> Response<Cursor<Vec<u8>>> {
    if *request.method() != Method::Get {
        return text(405, "method not allowed");
    }
    let path = request.url().split('?').next().unwrap_or_default();
    match path {
        "/api/sessions" => json(services::session_list(store)),
        "/api/prs" => json(services::pr_list(store)),
        _ => match ASSETS.iter().find(|(asset, ..)| *asset == path) {
            Some((_, content_type, body)) => {
                Response::from_data(body.to_vec()).with_header(header(content_type))
            }
            None => text(404, "not found"),
        },
    }
}

/// What the CLI prints for the same read: JSON and a newline, or `{error, help}` with
/// status 500.
fn json(result: anyhow::Result<impl Serialize>) -> Response<Cursor<Vec<u8>>> {
    let (status, body) = match result.and_then(|value| Ok(serde_json::to_string(&value)?)) {
        Ok(body) => (200, body),
        Err(err) => (
            500,
            serde_json::json!({
                "error": format!("{err:#}"),
                "help": services::STORE_ERROR_HELP,
            })
            .to_string(),
        ),
    };
    Response::from_data(format!("{body}\n"))
        .with_status_code(status)
        .with_header(header("application/json"))
}

fn text(status: u16, body: &str) -> Response<Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("text/plain; charset=utf-8"))
}

fn header(content_type: &str) -> Header {
    Header::from_bytes("Content-Type", content_type).expect("valid header")
}
