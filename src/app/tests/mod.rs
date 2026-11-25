#[cfg(all(feature = "elasticsearch", feature = "saved-views"))]
mod context;
#[cfg(feature = "elasticsearch")]
mod elasticsearch;
mod file_source;
mod modals;
#[cfg(feature = "sqlite")]
mod sqlite;
mod startup;
#[cfg(feature = "saved-views")]
mod view_saving;

use super::interactive::source_extent_label;
#[cfg(feature = "sqlite")]
use super::startup::prepare_app;
#[cfg(feature = "saved-views")]
use super::startup::prepare_saved_view_invocation;
use super::*;
#[cfg(any(feature = "sqlite", feature = "saved-views"))]
use crate::cli;
use crate::ops::filter::FilterKind;
#[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
use crate::output;
#[cfg(any(feature = "sqlite", feature = "saved-views"))]
use clap::Parser;
use crossterm::event::KeyModifiers;
use std::cell::Cell;
#[cfg(feature = "elasticsearch")]
use std::io::Read;
use std::io::{Seek, Write};
#[cfg(feature = "elasticsearch")]
use std::net::TcpListener;
#[cfg(feature = "saved-views")]
use std::path::PathBuf;
#[cfg(feature = "elasticsearch")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
#[cfg(feature = "elasticsearch")]
use std::time::Duration;
#[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
fn write_frozen_test_output(
    app: &App,
    format: output::OutputFormat,
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    let color = output::ColorOutput::Never;
    let projection = app
        .view
        .prepare_projection(output::requirements(format, color)?, &app.theme)?;
    output::write_prepared(format, color, &projection.output, None, writer)
}

#[cfg(feature = "elasticsearch")]
struct MockElasticsearchResponse {
    status: u16,
    body: &'static str,
    delay: std::time::Duration,
}

#[cfg(feature = "elasticsearch")]
impl MockElasticsearchResponse {
    fn ok(body: &'static str) -> Self {
        Self {
            status: 200,
            body,
            delay: std::time::Duration::ZERO,
        }
    }

    fn delayed(body: &'static str, delay: std::time::Duration) -> Self {
        Self {
            status: 200,
            body,
            delay,
        }
    }

    fn error(status: u16, body: &'static str) -> Self {
        Self {
            status,
            body,
            delay: std::time::Duration::ZERO,
        }
    }
}

#[cfg(feature = "elasticsearch")]
struct MockElasticsearch {
    endpoint: url::Url,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "elasticsearch")]
impl MockElasticsearch {
    fn start(responses: Vec<MockElasticsearchResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock Elasticsearch");
        listener.set_nonblocking(true).unwrap();
        let endpoint =
            url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker_requests = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            serve_mock_requests(listener, responses, &worker_requests, &worker_stop);
        });
        Self {
            endpoint,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn source(&self) -> ingest::source::InputSource {
        ingest::source::InputSource::Url(self.endpoint.clone())
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[cfg(feature = "elasticsearch")]
fn serve_mock_requests(
    listener: TcpListener,
    responses: Vec<MockElasticsearchResponse>,
    requests: &Mutex<Vec<String>>,
    stop: &AtomicBool,
) {
    let mut responses = responses.into_iter();
    while !stop.load(Ordering::Acquire) {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) => panic!("mock accept: {error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let Some(request) = read_mock_request(&mut stream) else {
            continue;
        };
        requests.lock().unwrap().push(request);
        let response = responses.next().expect("unexpected Elasticsearch request");
        write_mock_response(&mut stream, response);
    }
}

#[cfg(feature = "elasticsearch")]
fn read_mock_request(stream: &mut std::net::TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = read_mock_chunk(stream, &mut chunk, "request")?;
        request.extend_from_slice(&chunk[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = String::from_utf8_lossy(&request[..header_end]);
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();

    while request.len().saturating_sub(header_end) < content_length {
        let count = read_mock_chunk(stream, &mut chunk, "body")?;
        request.extend_from_slice(&chunk[..count]);
    }
    Some(String::from_utf8_lossy(&request).into_owned())
}

#[cfg(feature = "elasticsearch")]
fn read_mock_chunk(
    stream: &mut std::net::TcpStream,
    chunk: &mut [u8],
    phase: &str,
) -> Option<usize> {
    use std::io::ErrorKind;
    match stream.read(chunk) {
        Ok(0) => None,
        Ok(count) => Some(count),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => None,
        Err(error) => panic!("read mock {phase}: {error}"),
    }
}

#[cfg(feature = "elasticsearch")]
fn write_mock_response(stream: &mut std::net::TcpStream, response: MockElasticsearchResponse) {
    if !response.delay.is_zero() {
        std::thread::sleep(response.delay);
    }
    let reason = if response.status < 400 {
        "OK"
    } else {
        "Bad Request"
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.body.len(),
        response.body
    )
    .expect("mock response");
    stream.flush().expect("flush mock response");
}

#[cfg(feature = "elasticsearch")]
impl Drop for MockElasticsearch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.expect("mock Elasticsearch worker panicked");
            }
        }
    }
}

fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
    values
        .iter()
        .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
        .collect()
}

fn app_with_rows(rows: Vec<Vec<String>>) -> App {
    App {
        source: ingest::source::InputSource::Stdin,
        view: view::TableView::classify(rows, view::Viewport::new(10, 4)),
        popup: None,
        filter_prompt: None,
        column_info: None,
        source_modal: None,
        search_query: String::new(),
        keys: command::KeyInterpreter::default(),
        message: None,
        diagnostics: Vec::new(),
        theme: theme::default_theme(),
        #[cfg(feature = "saved-views")]
        saved_view: None,
        #[cfg(feature = "saved-views")]
        view_modal: None,
    }
}

fn app_for_source(path: std::path::PathBuf, options: ingest::OpenOptions) -> App {
    let opened = ingest::open_source(ingest::source::InputSource::Path(path.clone()), &options)
        .expect("open")
        .into_implicit_table()
        .expect("table");
    let mut app = app_with_rows(rows(&[&["placeholder"], &["value"]]));
    app.source = ingest::source::InputSource::Path(path);
    app.view =
        view::TableView::from_opened_table(opened, view::Viewport::new(8, 80)).expect("view");
    app.view
        .initialize_source_configuration(options)
        .expect("committed source");
    app
}

#[cfg(feature = "elasticsearch")]
fn app_for_elasticsearch(server: &MockElasticsearch, options: ingest::OpenOptions) -> App {
    let source = server.source();
    let opened = ingest::open_source(source.clone(), &options)
        .expect("open Elasticsearch")
        .into_implicit_table()
        .expect("Elasticsearch table");
    let mut app = app_with_rows(rows(&[&["placeholder"], &["value"]]));
    app.source = source;
    app.view =
        view::TableView::from_opened_table(opened, view::Viewport::new(8, 80)).expect("view");
    app.view
        .initialize_source_configuration(options)
        .expect("committed Elasticsearch source");
    app
}

#[cfg(feature = "sqlite")]
fn create_sqlite_database(path: &std::path::Path, statements: &[&str]) {
    let runtime = tokio::runtime::Runtime::new().expect("sqlite runtime");
    runtime.block_on(async {
        let database = turso::Builder::new_local(path.to_str().expect("utf8 path"))
            .build()
            .await
            .expect("sqlite database");
        let connection = database.connect().expect("sqlite connection");
        for statement in statements {
            connection
                .execute(statement, ())
                .await
                .expect("sqlite statement");
        }
    });
}

#[cfg(feature = "sqlite")]
fn sqlite_app(statements: &[&str], table: &str) -> (tempfile::TempDir, App) {
    let directory = tempfile::tempdir().expect("sqlite fixture");
    let path = directory.path().join("modal.db");
    create_sqlite_database(&path, statements);
    let options = ingest::OpenOptions {
        table: Some(table.to_owned()),
        ..ingest::OpenOptions::default()
    };
    (directory, app_for_source(path, options))
}

#[cfg(feature = "sqlite")]
fn sqlite_artifact_snapshot(path: &std::path::Path) -> Vec<(String, Option<Vec<u8>>)> {
    ["", "-journal", "-wal", "-shm", "-tshm", "-log"]
        .into_iter()
        .map(|suffix| {
            let mut artifact = path.as_os_str().to_os_string();
            artifact.push(suffix);
            (
                suffix.to_owned(),
                std::fs::read(std::path::PathBuf::from(artifact)).ok(),
            )
        })
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[cfg(feature = "saved-views")]
fn app_with_saved_view_target(rows: Vec<Vec<String>>, target_path: PathBuf) -> App {
    App {
        source: ingest::source::InputSource::Path(PathBuf::from("cat_shards.txt")),
        view: view::TableView::classify(rows, view::Viewport::new(10, 4)),
        popup: None,
        filter_prompt: None,
        column_info: None,
        source_modal: None,
        search_query: String::new(),
        keys: command::KeyInterpreter::default(),
        message: None,
        diagnostics: Vec::new(),
        theme: theme::default_theme(),
        saved_view: Some(SavedViewRuntime {
            source_path: None,
            target_path: Some(target_path),
            view_name: "cat_shards".to_owned(),
            explicit_locale: None,
        }),
        view_modal: None,
    }
}
