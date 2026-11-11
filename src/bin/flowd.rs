//! `flowd`: the daemon on the Flow CDE, run in the foreground. It serves the read API
//! (`GET /api/sessions`, `GET /api/prs`, the same JSON as `flow session list` and
//! `flow pr list`) and the embedded frontend, on `127.0.0.1` only. Every request reads the
//! Store fresh.

use std::io::Cursor;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use jiff::Timestamp;
use serde::Serialize;
use tiny_http::{Header, Method, Request, Response, Server};

use flow::store::Store;
use flow::{github, retention, services};

/// Serve flow's read API and web app on 127.0.0.1.
#[derive(Parser)]
#[command(name = "flowd")]
struct Args {
    /// The loopback port to listen on; 0 picks a free one.
    #[arg(long, env = "FLOWD_PORT", default_value_t = 7171)]
    port: u16,
    /// The hard timeout on each `gh` call, in seconds.
    #[arg(long, env = "FLOWD_GH_TIMEOUT", default_value_t = github::TIMEOUT.as_secs(), hide = true)]
    gh_timeout: u64,
}

/// How often the retention sweep runs after the one at start.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// The frontend bundle (`web/dist`), embedded at build time: (path, content type, body).
const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/",
        "text/html; charset=utf-8",
        include_bytes!("../../web/dist/index.html"),
    ),
    (
        "/favicon.svg",
        "image/svg+xml",
        include_bytes!("../../web/dist/favicon.svg"),
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
    let sweep_store = store.clone();
    thread::spawn(move || sweep_loop(&sweep_store));
    thread::spawn({
        let store = store.clone();
        let timeout = Duration::from_secs(args.gh_timeout);
        move || poll_loop(&store, timeout)
    });
    for request in server.incoming_requests() {
        let response = respond(&store, &request);
        if let Err(err) = request.respond(response) {
            eprintln!("flowd: cannot send a response: {err}");
        }
    }
    ExitCode::SUCCESS
}

/// Runs the retention sweep now and then every [`SWEEP_EVERY`], forever. Errors go to stderr.
fn sweep_loop(store: &Store) {
    loop {
        if let Err(err) = retention::sweep(store, Timestamp::now()) {
            eprintln!("flowd: retention sweep failed: {err:#}");
        }
        thread::sleep(SWEEP_EVERY);
    }
}

/// The PR Writer: polls at once, then every [`github::INTERVAL`]. Errors go to stderr; one
/// summary line per cycle goes to stdout.
fn poll_loop(store: &Store, timeout: Duration) -> ! {
    loop {
        let started = Instant::now();
        let cycle = github::poll_prs(store, timeout);
        for err in &cycle.errors {
            eprintln!("flowd: pr poll: {err:#}");
        }
        println!(
            "flowd: pr poll wrote {} PRs in {:.1}s for {} points, {} errors",
            cycle.written,
            started.elapsed().as_secs_f64(),
            cycle.cost,
            cycle.errors.len()
        );
        thread::sleep(github::INTERVAL.saturating_sub(started.elapsed()));
    }
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
