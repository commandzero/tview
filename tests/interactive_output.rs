#![cfg(unix)]
// PTY process setup and signal delivery require libc; production code stays denied.
#![allow(unsafe_code)]

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(feature = "elasticsearch")]
#[path = "support/elasticrc.rs"]
pub mod elasticrc;
#[cfg(feature = "elasticsearch")]
#[path = "support/elasticsearch_mock.rs"]
pub mod elasticsearch_mock;
static PTY_LOCK: Mutex<()> = Mutex::new(());

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn run_in_pty(command: &str, keys: &[u8]) -> Output {
    let _guard = PTY_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut script = Command::new("script");
    #[cfg(target_os = "linux")]
    // util-linux script needs -e to propagate the command's exit status.
    script.args(["-q", "-e", "-c", command, "/dev/null"]);
    #[cfg(not(target_os = "linux"))]
    script.args(["-q", "/dev/null", "/bin/sh", "-c", command]);
    let mut child = script
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn script");
    let mut script_stdout = child.stdout.take().expect("script stdout");
    let stdout_reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        script_stdout
            .read_to_end(&mut output)
            .expect("read script stdout");
        output
    });
    let mut script_stderr = child.stderr.take().expect("script stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        script_stderr
            .read_to_end(&mut output)
            .expect("read script stderr");
        output
    });
    std::thread::sleep(Duration::from_millis(300));
    let mut terminal_input = child.stdin.take().expect("script stdin");
    terminal_input.write_all(keys).expect("send keys");
    let status = child.wait().expect("wait for script");
    drop(terminal_input);
    let stdout = stdout_reader.join().expect("join stdout reader");
    let stderr = stderr_reader.join().expect("join stderr reader");
    Output {
        status,
        stdout,
        stderr,
    }
}

#[test]
fn help_uses_terminal_colors_and_respects_no_color() {
    let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
    for flag in ["-h", "--help"] {
        for no_color in [false, true] {
            let setting = if no_color { "NO_COLOR=1" } else { "" };
            let command = format!(
                "env -u NO_COLOR -u CLICOLOR_FORCE -u CLICOLOR TERM=xterm-256color {setting} {binary} {flag}"
            );
            let output = run_in_pty(&command, b"");
            assert!(output.status.success(), "output: {output:?}");
            assert!(output.stderr.is_empty(), "output: {output:?}");
            let help = String::from_utf8(output.stdout).expect("UTF-8 help");
            assert!(help.contains("Usage:"));
            assert!(help.contains("--table-color"));
            if no_color {
                assert!(!help.contains("\x1b["), "help: {help:?}");
            } else {
                for color in ["\x1b[35m", "\x1b[36m", "\x1b[32m"] {
                    assert!(help.contains(color), "missing {color:?} in help: {help:?}");
                }
            }
        }
    }
}

#[test]
fn interactive_export_applies_edits_and_waits_for_late_stdin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output_path = dir.path().join("output.txt");
    let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
    let destination = shell_quote(&output_path);
    let command = format!(
        "(printf 'A,B\\n1,2\\n'; sleep 1; printf '3,4\\n') | {binary} -i -o table - > {destination}"
    );

    let output = run_in_pty(&command, b"chjq");
    assert!(output.status.success(), "output: {output:?}");
    assert_eq!(
        std::fs::read_to_string(output_path).expect("output"),
        "B\n2\n4\n"
    );
}

#[test]
fn post_start_ingestion_failure_does_not_export_partial_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output_path = dir.path().join("output.txt");
    let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
    let destination = shell_quote(&output_path);
    let command = format!(
        "(printf '[\\n{{\"a\":1}},\\n'; sleep 1; printf '{{broken]\\n') | {binary} --format json -i -o table - > {destination}"
    );

    let output = run_in_pty(&command, b"q");
    assert!(!output.status.success(), "output: {output:?}");
    assert_eq!(std::fs::read(output_path).expect("output"), b"");
}

#[test]
fn cancelled_interactive_transform_does_not_export() {
    let _guard = PTY_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("tempdir");
    let input_path = dir.path().join("input.csv");
    let output_path = dir.path().join("output.txt");
    let pid_path = dir.path().join("tview.pid");
    std::fs::write(&input_path, "A,B\n1,2\n").expect("input");
    let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
    let input = shell_quote(&input_path);
    let destination = shell_quote(&output_path);
    let pid_file = shell_quote(&pid_path);
    let command =
        format!("echo $$ > {pid_file}; exec {binary} -i -o table {input} > {destination}");
    let mut script = Command::new("script");
    #[cfg(target_os = "linux")]
    script.args(["-q", "-e", "-c", &command, "/dev/null"]);
    #[cfg(not(target_os = "linux"))]
    script.args(["-q", "/dev/null", "/bin/sh", "-c", &command]);
    let child = script
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn script");

    let pid = (0..100)
        .find_map(|_| {
            let pid = std::fs::read_to_string(&pid_path)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok());
            if pid.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            pid
        })
        .expect("tview pid");
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: the PID was emitted by the test's child shell and SIGTERM has no
    // memory-safety preconditions.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);

    let output = child.wait_with_output().expect("wait for script");
    assert!(!output.status.success(), "output: {output:?}");
    assert_eq!(std::fs::read(output_path).expect("output"), b"");
}

#[test]
fn explicit_view_only_mode_does_not_export() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input_path = dir.path().join("input.csv");
    let output_path = dir.path().join("output.txt");
    std::fs::write(&input_path, "A,B\n1,2\n").expect("input");
    let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
    let input = shell_quote(&input_path);
    let destination = shell_quote(&output_path);
    let command = format!("{binary} -i {input} > {destination}");

    let output = run_in_pty(&command, b"q");
    assert!(output.status.success(), "output: {output:?}");
    assert_eq!(std::fs::read(output_path).expect("output"), b"");
}

#[test]
fn terminal_stdout_selects_automatic_view_only_tui() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input_path = dir.path().join("input.csv");
    std::fs::write(&input_path, "A,B\n1,2\n").expect("input");
    let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
    let input = shell_quote(&input_path);

    let output = run_in_pty(&format!("{binary} {input}"), b"q");
    assert!(output.status.success(), "output: {output:?}");
    assert!(
        output
            .stdout
            .windows(b"\x1b[?1049h".len())
            .any(|window| window == b"\x1b[?1049h"),
        "automatic mode did not enter the alternate screen"
    );
}

#[test]
fn interactive_mode_without_a_controlling_terminal_fails_without_output() {
    let _guard = PTY_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("tempdir");
    let input_path = dir.path().join("input.csv");
    std::fs::write(&input_path, "A,B\n1,2\n").expect("input");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tview"));
    command
        .args(["-i", "-o", "table"])
        .arg(input_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: `pre_exec` runs in the child immediately before exec. `setsid`
    // has no Rust memory-safety preconditions and intentionally detaches the
    // child from this test runner's controlling terminal.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }

    let output = command.output().expect("run detached tview");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[cfg(feature = "elasticsearch")]
mod elasticrc_contexts {
    use super::*;
    use crate::elasticrc::{
        authorization, no_secrets, request_query, service, success, Fixture, DISCOVERY, FIELD_CAPS,
        MAPPING, QUERY, ROWS,
    };
    use crate::elasticsearch_mock::{Response, Server};
    use parking_lot::Mutex as ScreenMutex;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Instant;

    struct Terminal {
        child: std::process::Child,
        input: std::process::ChildStdin,
        screen: Arc<ScreenMutex<Vec<u8>>>,
        rendered: Arc<ScreenMutex<vt100::Parser>>,
        reader: Option<std::thread::JoinHandle<()>>,
        errors: Arc<ScreenMutex<Vec<u8>>>,
        error_reader: Option<std::thread::JoinHandle<()>>,
    }

    impl Terminal {
        fn start(command: &str) -> Self {
            // `script` inherits a zero-sized terminal when the test runner's
            // streams are pipes. Set the slave window before starting Tview.
            let command = format!("stty rows 40 columns 120 < /dev/tty && exec {command}");
            let mut script = Command::new("script");
            #[cfg(target_os = "linux")]
            script.args(["-q", "-e", "-f", "-c", &command, "/dev/null"]);
            #[cfg(not(target_os = "linux"))]
            script.args(["-q", "-F", "/dev/null", "/bin/sh", "-c", &command]);
            script.env("SHELL", "/bin/sh");
            let mut child = script
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let input = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            let screen = Arc::new(ScreenMutex::new(Vec::new()));
            let collected = screen.clone();
            let rendered = Arc::new(ScreenMutex::new(vt100::Parser::new(40, 120, 0)));
            let parsed = rendered.clone();
            let reader = std::thread::spawn(move || {
                let mut buffer = [0; 4096];
                loop {
                    let count = stdout.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    collected.lock().extend_from_slice(&buffer[..count]);
                    parsed.lock().process(&buffer[..count]);
                }
            });
            let errors = Arc::new(ScreenMutex::new(Vec::new()));
            let collected_errors = errors.clone();
            let error_reader = std::thread::spawn(move || {
                let mut buffer = [0; 4096];
                loop {
                    let count = stderr.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    collected_errors.lock().extend_from_slice(&buffer[..count]);
                }
            });
            Self {
                child,
                input,
                screen,
                rendered,
                reader: Some(reader),
                errors,
                error_reader: Some(error_reader),
            }
        }

        fn send(&mut self, keys: &[u8]) {
            self.input.write_all(keys).unwrap();
            self.input.flush().unwrap();
        }

        fn contains(&self, text: &str) -> bool {
            self.rendered.lock().screen().contents().contains(text)
        }

        fn transcript_contains(&self, text: &str) -> bool {
            String::from_utf8_lossy(&self.screen.lock()).contains(text)
        }

        fn open_modal(&mut self, key: &[u8], title: &str) {
            self.send(key);
            self.wait_for("modal opening", || self.contains(title));
        }

        fn close_modal(&mut self, title: &str) {
            self.send(b"\x1b");
            self.wait_for("modal closing", || !self.contains(title));
        }

        fn reduce_source_limit(&mut self) {
            self.open_modal(b"u", "Source Configuration");
            self.send(b"-");
            self.wait_for("limit draft", || self.contains("Limit: 900"));
            self.send(b"\r");
            self.wait_for("source modal closing", || {
                !self.contains("Source Configuration")
            });
        }

        fn show_query(&mut self) {
            self.open_modal(b"p", "Source Query");
            self.close_modal("Source Query");
        }

        #[cfg(feature = "saved-views")]
        fn save(&mut self) {
            self.open_modal(b"v", "Save");
            self.send(b"s");
        }

        fn wait_for(&self, reason: &str, mut predicate: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(8);
            while !predicate() {
                if self
                    .reader
                    .as_ref()
                    .is_some_and(|reader| reader.is_finished())
                {
                    // Recheck after EOF: the reader may have appended the
                    // awaited final frame between the first check and EOF.
                    assert!(
                        predicate(),
                        "terminal closed while waiting for {reason}; terminal: {}; stderr: {}",
                        self.rendered.lock().screen().contents(),
                        String::from_utf8_lossy(&self.errors.lock())
                    );
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {reason}; terminal: {}; stderr: {}",
                    self.rendered.lock().screen().contents(),
                    String::from_utf8_lossy(&self.errors.lock())
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn finish(mut self) -> Output {
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "PTY child did not exit; terminal: {}; stderr: {}",
                    self.rendered.lock().screen().contents(),
                    String::from_utf8_lossy(&self.errors.lock())
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            self.reader.take().unwrap().join().unwrap();
            self.error_reader.take().unwrap().join().unwrap();
            let stderr = self.errors.lock().clone();
            let stdout = self.screen.lock().clone();
            Output {
                status,
                stdout,
                stderr,
            }
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            self.child.kill().ok();
            self.child.wait().ok();
        }
    }

    fn command(fixture: &Fixture, args: &str, destination: Option<&Path>) -> String {
        let binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_tview")));
        let redirect = destination
            .map(|path| format!(" > {}", shell_quote(path)))
            .unwrap_or_default();
        format!(
            "{} {binary} --color never -i {args}{redirect}",
            fixture.shell_environment()
        )
    }

    struct ResolverLifetime {
        exited: std::sync::mpsc::Receiver<std::io::Result<()>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        reader: Option<std::thread::JoinHandle<()>>,
    }

    impl ResolverLifetime {
        fn start(path: &Path) -> Self {
            use std::os::unix::fs::OpenOptionsExt;
            use std::sync::atomic::Ordering;
            let created = Command::new("mkfifo")
                .arg(path)
                .output()
                .expect("create resolver lifetime FIFO");
            assert!(
                created.status.success(),
                "mkfifo: {}",
                String::from_utf8_lossy(&created.stderr)
            );
            // Open before spawning the resolver, without blocking when startup
            // fails before it ever opens its writer.
            let mut fifo = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
                .unwrap();
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let reader_stop = stop.clone();
            let (sender, exited) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let mut connected = false;
                let mut buffer = [0_u8; 16];
                while !reader_stop.load(Ordering::Acquire) {
                    match fifo.read(&mut buffer) {
                        Ok(0) if connected => {
                            sender.send(Ok(())).ok();
                            return;
                        }
                        Ok(0) => {}
                        Ok(_) => connected = true,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) => {
                            sender.send(Err(error)).ok();
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            Self {
                exited,
                stop,
                reader: Some(reader),
            }
        }

        fn wait_for_exit(&self) {
            self.exited
                .recv_timeout(Duration::from_secs(8))
                .expect("cancelled resolver retained lifetime FIFO")
                .expect("read resolver lifetime FIFO");
        }
    }

    impl Drop for ResolverLifetime {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            self.reader
                .take()
                .unwrap()
                .join()
                .expect("join resolver lifetime reader");
        }
    }

    #[test]
    fn delayed_initial_context_resolver_keeps_terminal_quit_responsive() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let started = fixture.root.path().join("resolver-started");
        let release = fixture.root.path().join("resolver-release");
        let completed = fixture.root.path().join("resolver-completed");
        let lifetime_path = fixture.root.path().join("resolver-lifetime.fifo");
        let lifetime = ResolverLifetime::start(&lifetime_path);
        let resolver =
            fixture.resolver(
                "delayed",
                &format!(
            "exec 3>{}\nprintf ready >&3\ntouch {}\nattempts=0\nwhile [ ! -f {} ]; do\n  attempts=$((attempts + 1))\n  [ \"$attempts\" -lt 160 ] || exit 1\n  sleep 0.05\ndone\ntouch {}\nprintf delayed-secret",
            shell_quote(&lifetime_path), shell_quote(&started), shell_quote(&release), shell_quote(&completed)),
            );
        fixture.write("production", json!({"production": service("http://127.0.0.1:1", Some(json!({"api_key": resolver})))}));
        let mut terminal = Terminal::start(&command(
            &fixture,
            ".es:// --query 'FROM logs-* | KEEP message'",
            None,
        ));
        terminal.wait_for("resolver starting", || started.exists());
        terminal.send(b"q");
        terminal.wait_for("terminal restored while resolver remains blocked", || {
            terminal.transcript_contains("\x1b[?1049l")
        });
        assert!(
            !completed.exists(),
            "input only handled after resolver finished"
        );
        std::fs::write(&release, "").unwrap();
        // Kernel EOF covers normal exit and every termination signal, including
        // SIGKILL (which cannot execute shell traps). Inherited sleep writers
        // live for at most their existing 50ms barrier interval.
        lifetime.wait_for_exit();
        let output = terminal.finish();
        success(&output);
        no_secrets(&output, &["delayed-secret"]);
    }

    #[test]
    fn context_picker_mapping_query_replacement_and_reload_share_one_resolved_connection() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let counter = fixture.root.path().join("counter");
        let resolver = fixture.resolver(
            "counter-resolver",
            &format!(
                "printf 'run\\n' >> {}\nprintf picker-secret",
                shell_quote(&counter)
            ),
        );
        let server = Server::start(vec![
            Response::ok(DISCOVERY),
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(ROWS),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["replacement-row"]]}"#,
            ),
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["reloaded-row"]]}"#,
            ),
        ]);
        let changed = Server::start(vec![Response::ok(
            r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["fresh-context-row"]]}"#,
        )]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), Some(json!({"api_key": resolver})))}),
        );
        let destination = fixture.root.path().join("output");
        let mut terminal =
            Terminal::start(&command(&fixture, ".es:// -o table", Some(&destination)));
        terminal.wait_for("picker", || terminal.contains("logs-a"));
        terminal.send(b"\r");
        terminal.wait_for("selected result", || terminal.contains("context-row"));
        let changed_resolver = fixture.resolver(
            "counter-resolver",
            &format!(
                "printf 'run\\n' >> {}\nprintf changed-picker-secret",
                shell_quote(&counter)
            ),
        );
        fixture.write("staging", json!({
            "production": service(server.endpoint(), Some(json!({"api_key": changed_resolver}))),
            "staging": service(changed.endpoint(), Some(json!({"api_key": changed_resolver}))),
        }));
        terminal.reduce_source_limit();
        terminal.wait_for("replacement on pinned connection", || {
            server.requests().len() == 5
        });
        terminal.wait_for("replacement activated", || {
            terminal.contains("replacement-row")
        });
        terminal.send(b"r");
        terminal.wait_for("reload mappings/query on pinned connection", || {
            server.requests().len() == 8
        });
        terminal.wait_for("reload activated", || terminal.contains("reloaded-row"));
        terminal.send(b"q");
        success(&terminal.finish());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap(),
            "run\n",
            "resolver ran again during discovery/mapping/replacement/reload"
        );
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"message\nreloaded-row\n"
        );
        assert_eq!(server.requests().len(), 8);
        assert!(
            changed.requests().is_empty(),
            "active invocation followed changed current context"
        );
        for request in server.requests() {
            assert_eq!(authorization(&request), Some("ApiKey picker-secret"));
        }
        let fresh = fixture.run(&[".es://", "--query", QUERY]);
        success(&fresh);
        assert_eq!(fresh.stdout, b"message\nfresh-context-row\n");
        assert_eq!(
            std::fs::read_to_string(counter).unwrap(),
            "run\nrun\n",
            "new invocation did not resolve changed credentials"
        );
        assert_eq!(
            authorization(&changed.requests()[0]),
            Some("ApiKey changed-picker-secret")
        );
    }

    #[test]
    fn context_query_replacement_and_reload_pin_current_context_and_credentials() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let counter = fixture.root.path().join("counter");
        let resolver = fixture.resolver(
            "counter-resolver",
            &format!(
                "printf 'run\\n' >> {}\nprintf pinned-secret",
                shell_quote(&counter)
            ),
        );
        let original = Server::start(vec![
            Response::ok(ROWS),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["context-replacement-row"],["excluded-sentinel"]]}"#,
            ),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["context-reloaded-row"],["excluded-sentinel"]]}"#,
            ),
        ]);
        let changed = Server::start(vec![Response::ok(
            r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["new-invocation"]]}"#,
        )]);
        fixture.write(
            "production",
            json!({"production": service(original.endpoint(), Some(json!({"api_key": resolver})))}),
        );
        let destination = fixture.root.path().join("output");
        let mut terminal = Terminal::start(&command(
            &fixture,
            ".es:// --query 'FROM logs-* | KEEP message' -o table",
            Some(&destination),
        ));
        terminal.wait_for("initial rows", || terminal.contains("context-row"));
        fixture.write("staging", json!({"staging": service(changed.endpoint(), Some(json!({"api_key": "changed-secret"})))}));
        // Local view state must survive compatible source replacement/reload.
        terminal.open_modal(b"f", "Filter in");
        terminal.send(b"context\r");
        terminal.wait_for("local filter modal closing", || {
            !terminal.contains("Filter in")
        });
        terminal.reduce_source_limit();
        terminal.wait_for("query replacement request", || {
            original.requests().len() == 2
        });
        terminal.wait_for("replacement result", || {
            terminal.contains("context-replacement-row")
        });
        terminal.send(b"r");
        terminal.wait_for("reload request", || original.requests().len() == 3);
        terminal.wait_for("reload result", || {
            terminal.contains("context-reloaded-row")
        });
        terminal.send(b"q");
        let output = terminal.finish();
        success(&output);
        no_secrets(&output, &["pinned-secret", "changed-secret"]);
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "run\n");
        assert!(changed.requests().is_empty(), "reload switched context");
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"message\ncontext-reloaded-row\n",
            "compatible local filter was lost"
        );
        assert!(request_query(&original.requests()[1]).ends_with("| LIMIT 901"));
        assert!(request_query(&original.requests()[2]).ends_with("| LIMIT 901"));
        for request in original.requests() {
            assert_eq!(authorization(&request), Some("ApiKey pinned-secret"));
        }
        let fresh = fixture.run(&[".es://", "--query", QUERY]);
        success(&fresh);
        assert_eq!(fresh.stdout, b"message\nnew-invocation\n");
        assert_eq!(
            authorization(&changed.requests()[0]),
            Some("ApiKey changed-secret")
        );
    }

    #[cfg(feature = "saved-views")]
    fn saved_yaml(fixture: &Fixture) -> (std::path::PathBuf, String) {
        let views = fixture.xdg.join("tview/views");
        let files = std::fs::read_dir(views)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1, "expected one generated view");
        let yaml = std::fs::read_to_string(&files[0]).unwrap();
        (files[0].clone(), yaml)
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saving_contexts_canonicalizes_alias_and_never_persists_resolved_connection() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut filenames = std::collections::HashMap::new();
        for (source, identity) in [
            (".es://", ".elasticsearch://"),
            (".elasticsearch://", ".elasticsearch://"),
            (
                ".production.us-west.es://",
                ".production.us-west.elasticsearch://",
            ),
            (
                ".production.us-west.elasticsearch://",
                ".production.us-west.elasticsearch://",
            ),
        ] {
            let fixture = Fixture::new();
            let server = Server::start(vec![Response::ok(ROWS), Response::ok(ROWS)]);
            let resolver = fixture.resolver("secret-resolver", "printf save-secret-sentinel");
            fixture.write("production.us-west", json!({"production.us-west": service(server.endpoint(), Some(json!({"api_key": resolver})))}));
            let mut terminal = Terminal::start(&command(
                &fixture,
                &format!("{source} --query 'FROM logs-* | KEEP message'"),
                None,
            ));
            terminal.wait_for("initial result", || terminal.contains("context-row"));
            terminal.show_query();
            terminal.save();
            terminal.wait_for("generated view", || {
                fixture.xdg.join("tview/views").is_dir()
                    && std::fs::read_dir(fixture.xdg.join("tview/views"))
                        .unwrap()
                        .next()
                        .is_some()
            });
            terminal.close_modal("Save");
            terminal.send(b"q");
            let output = terminal.finish();
            success(&output);
            no_secrets(
                &output,
                &["save-secret-sentinel", server.endpoint(), "$(cmd:"],
            );
            let (path, yaml) = saved_yaml(&fixture);
            let document: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
            assert_eq!(document["filenames"][0].as_str(), Some(identity));
            assert_eq!(document["source"]["query"].as_str(), Some(QUERY));
            let filename = path.file_name().unwrap().to_owned();
            if let Some(previous) = filenames.insert(identity, filename.clone()) {
                assert_eq!(previous, filename, "alias changed generated filename");
            }
            for forbidden in [
                "save-secret-sentinel",
                server.endpoint(),
                "$(cmd:",
                "authorization",
                "api_key",
            ] {
                assert!(!yaml.contains(forbidden), "{forbidden} in saved YAML");
            }
            // The saved current selector follows future current context; aliases
            // select this same saved view, including its committed native query.
            let alias = identity.replace(".elasticsearch://", ".es://");
            let refreshed = Server::start(vec![Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["saved-current-refreshed"]]}"#,
            )]);
            let expected = if identity == ".elasticsearch://" {
                fixture.write(
                    "staging",
                    json!({"staging": service(refreshed.endpoint(), None)}),
                );
                b"message\nsaved-current-refreshed\n".as_slice()
            } else {
                b"message\ncontext-row\n".as_slice()
            };
            let saved = fixture.run(&[&alias]);
            no_secrets(
                &saved,
                &["save-secret-sentinel", server.endpoint(), "$(cmd:"],
            );
            assert!(
                saved.status.success(),
                "saved-view roundtrip failed: source={source:?}; alias={alias:?}; identity={identity:?}; path={}; yaml={yaml}; status={:?}; stderr={}",
                path.display(), saved.status, String::from_utf8_lossy(&saved.stderr)
            );
            assert_eq!(saved.stdout, expected);
        }
        assert_eq!(filenames.len(), 2);
        let mut names = filenames.values();
        assert_ne!(
            names.next().unwrap(),
            names.next().unwrap(),
            "named/current selectors shared a view filename"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn failed_context_source_query_keeps_committed_save_and_blocks_final_export() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let server = Server::start(vec![
            Response::ok(ROWS),
            Response::error(400, r#"{"error":{"reason":"secret-error-sentinel"}}"#),
        ]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        let destination = fixture.root.path().join("output");
        let mut terminal = Terminal::start(&command(
            &fixture,
            ".production.es:// --query 'FROM logs-* | KEEP message' -o table",
            Some(&destination),
        ));
        terminal.wait_for("initial rows", || terminal.contains("context-row"));
        terminal.reduce_source_limit();
        terminal.wait_for("failed replacement request", || {
            server.requests().len() == 2
        });
        terminal.wait_for("query error", || terminal.contains("failed"));
        terminal.save();
        terminal.wait_for("committed view saved", || {
            fixture.xdg.join("tview/views").is_dir()
                && std::fs::read_dir(fixture.xdg.join("tview/views"))
                    .unwrap()
                    .next()
                    .is_some()
        });
        terminal.close_modal("Save");
        terminal.send(b"q");
        let output = terminal.finish();
        assert!(!output.status.success());
        assert!(
            std::fs::read(destination).unwrap().is_empty(),
            "failed latest activation exported previous rows"
        );
        no_secrets(&output, &["secret-error-sentinel"]);
        let (_, yaml) = saved_yaml(&fixture);
        let document: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(
            document["filenames"][0].as_str(),
            Some(".production.elasticsearch://")
        );
        assert_eq!(document["source"]["query"].as_str(), Some(QUERY));
        assert_eq!(
            document["source"]["limit"].as_u64(),
            Some(1000),
            "failed candidate limit persisted"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saving_distinct_context_patterns_does_not_overwrite_another_view() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let server = Server::start(vec![
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(ROWS),
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(ROWS),
        ]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        let views = fixture.xdg.join("tview/views");
        // Discover both sources before saving: a loaded wildcard-matched view
        // intentionally saves back to its selected file, not a generated path.
        let terminals = ["logs-*", "logs-?"].map(|suffix| {
            let terminal = Terminal::start(&command(
                &fixture,
                &format!("'.production.es://{suffix}'"),
                None,
            ));
            terminal.wait_for("context result", || terminal.contains("context-row"));
            terminal
        });
        let mut first_saved = None;
        for (suffix, mut terminal) in ["logs-*", "logs-?"].into_iter().zip(terminals) {
            let identity = format!(".production.elasticsearch://{suffix}");
            terminal.save();
            terminal.wait_for("separately saved canonical identity", || {
                std::fs::read_dir(&views).is_ok_and(|entries| {
                    entries.filter_map(Result::ok).any(|entry| {
                        std::fs::read_to_string(entry.path())
                            .ok()
                            .and_then(|yaml| yaml_serde::from_str::<yaml_serde::Value>(&yaml).ok())
                            .is_some_and(|document| {
                                document["filenames"][0].as_str() == Some(identity.as_str())
                                    && document["source"]["table"].as_str() == Some(suffix)
                            })
                    })
                })
            });
            terminal.close_modal("Save");
            terminal.send(b"q");
            success(&terminal.finish());
            if first_saved.is_none() {
                first_saved = Some(saved_yaml(&fixture));
            }
        }
        let (first_path, first_yaml) = first_saved.unwrap();
        assert_eq!(std::fs::read_to_string(&first_path).unwrap(), first_yaml);
        let second_path = std::fs::read_dir(&views)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                let yaml = std::fs::read_to_string(path).unwrap();
                let document: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
                document["filenames"][0].as_str() == Some(".production.elasticsearch://logs-?")
            })
            .expect("second context identity has its own saved view");
        assert_ne!(first_path, second_path);
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saving_suffix_context_preserves_literal_canonical_identity_and_committed_table() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let server = Server::start(vec![
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(ROWS),
        ]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        let mut terminal = Terminal::start(&command(&fixture, ".production.es://logs-*", None));
        terminal.wait_for("initial rows", || terminal.contains("context-row"));
        terminal.save();
        terminal.wait_for("saved suffix view", || {
            fixture.xdg.join("tview/views").is_dir()
                && std::fs::read_dir(fixture.xdg.join("tview/views"))
                    .unwrap()
                    .next()
                    .is_some()
        });
        terminal.close_modal("Save");
        terminal.send(b"q");
        success(&terminal.finish());
        let (_, yaml) = saved_yaml(&fixture);
        let document: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(
            document["filenames"][0].as_str(),
            Some(".production.elasticsearch://logs-*")
        );
        assert_eq!(document["source"]["table"].as_str(), Some("logs-*"));
        assert!(!yaml.contains(server.endpoint()));
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn saving_during_pending_context_query_uses_committed_source_until_activation() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let server = Server::start(vec![
            Response::ok(ROWS),
            Response::delayed(ROWS, Duration::from_secs(2)),
        ]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), None)}),
        );
        let destination = fixture.root.path().join("output");
        let mut terminal = Terminal::start(&command(
            &fixture,
            ".es:// --query 'FROM logs-* | KEEP message' -o table",
            Some(&destination),
        ));
        terminal.wait_for("initial rows", || terminal.contains("context-row"));
        terminal.reduce_source_limit();
        terminal.wait_for("pending request", || server.requests().len() == 2);
        terminal.save();
        terminal.wait_for("saved pending view", || {
            fixture.xdg.join("tview/views").is_dir()
                && std::fs::read_dir(fixture.xdg.join("tview/views"))
                    .unwrap()
                    .next()
                    .is_some()
        });
        let (_, yaml) = saved_yaml(&fixture);
        let document: yaml_serde::Value = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(
            document["source"]["limit"].as_u64(),
            Some(1000),
            "pending candidate committed early"
        );
        terminal.close_modal("Save");
        terminal.send(b"q");
        success(&terminal.finish());
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"message\ncontext-row\n"
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn context_save_failure_is_safe_and_does_not_corrupt_committed_final_export() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let server = Server::start(vec![Response::ok(ROWS)]);
        fixture.write("production", json!({"production": service(server.endpoint(), Some(json!({"api_key": "save-failure-secret"})))}));
        let destination = fixture.root.path().join("output");
        let mut terminal = Terminal::start(&command(
            &fixture,
            ".es:// --query 'FROM logs-* | KEEP message' -o table",
            Some(&destination),
        ));
        terminal.wait_for("initial rows", || terminal.contains("context-row"));
        // A regular file cannot become the save directory on any platform,
        // including root-run tests where permission-only fixtures are ineffective.
        let views = fixture.xdg.join("tview/views");
        if views.is_dir() {
            std::fs::remove_dir(&views).unwrap();
        }
        std::fs::create_dir_all(views.parent().unwrap()).unwrap();
        std::fs::write(&views, "not a directory").unwrap();
        terminal.save();
        terminal.wait_for("save failure", || terminal.contains("failed to save"));
        terminal.close_modal("Save");
        terminal.send(b"q");
        let output = terminal.finish();
        success(&output);
        no_secrets(&output, &["save-failure-secret", server.endpoint()]);
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"message\ncontext-row\n"
        );
        assert_eq!(std::fs::read_to_string(views).unwrap(), "not a directory");
    }

    #[test]
    fn suffixed_context_native_query_replacement_survives_reload_and_redacts_success_warnings() {
        let _guard = PTY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::new();
        let counter = fixture.root.path().join("suffix-resolver-counter");
        let resolver = fixture.resolver(
            "suffix-resolver",
            &format!(
                "printf 'run\\n' >> {}\nprintf resolved-warning-secret-sentinel",
                shell_quote(&counter)
            ),
        );
        let server = Server::start(vec![
            Response::ok(MAPPING),
            Response::ok(FIELD_CAPS),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["context-row"]],"is_partial":true,"warnings":["resolved-warning-secret-sentinel"]}"#,
            ),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["native-replacement-row"]]}"#,
            ),
            Response::ok(
                r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["native-reloaded-row"]]}"#,
            ),
        ]);
        let changed = Server::start(vec![]);
        fixture.write(
            "production",
            json!({"production": service(server.endpoint(), Some(json!({"api_key": resolver})))}),
        );
        let destination = fixture.root.path().join("output");
        let mut terminal = Terminal::start(&command(
            &fixture,
            ".production.es://logs-a -o table",
            Some(&destination),
        ));
        terminal.wait_for("suffix result", || terminal.contains("context-row"));
        terminal.show_query();
        fixture.write("production", json!({"production": service(changed.endpoint(), Some(json!({"api_key": "changed-secret-sentinel"})))}));
        let initial = request_query(&server.requests()[2]);
        let base = initial
            .strip_suffix("\n| LIMIT 1001")
            .expect("initial bounded ES|QL");
        let replacement = "FROM replacement-logs | KEEP message";
        terminal.open_modal(b"u", "Source Configuration");
        terminal.send(b"q");
        terminal.wait_for("native query editor", || {
            terminal.contains("Native query editor:")
        });
        let mut keys = vec![127_u8; base.chars().count()];
        keys.extend_from_slice(replacement.as_bytes());
        terminal.send(&keys);
        terminal.wait_for("edited native query", || terminal.contains(replacement));
        terminal.send(b"\r");
        terminal.wait_for("native query staged", || {
            !terminal.contains("Native query editor:")
        });
        terminal.send(b"\r");
        terminal.wait_for("source modal closing", || {
            !terminal.contains("Source Configuration")
        });
        terminal.wait_for("native query replacement rows", || {
            terminal.contains("native-replacement-row")
        });
        terminal.send(b"r");
        terminal.wait_for("reloaded native query rows", || {
            terminal.contains("native-reloaded-row")
        });
        terminal.show_query();
        terminal.send(b"q");
        let output = terminal.finish();
        success(&output);
        no_secrets(
            &output,
            &[
                "resolved-warning-secret-sentinel",
                "changed-secret-sentinel",
            ],
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("partial"),
            "partial-result indication was lost"
        );
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"message\nnative-reloaded-row\n"
        );
        let requests = server.requests();
        assert_eq!(
            requests.len(),
            5,
            "reload returned to positional suffix mapping/discovery"
        );
        let expected = format!("{replacement}\n| LIMIT 1001");
        assert_eq!(request_query(&requests[3]), expected);
        assert_eq!(
            request_query(&requests[4]),
            expected,
            "reload replaced committed query with positional suffix"
        );
        for request in requests {
            assert_eq!(
                authorization(&request),
                Some("ApiKey resolved-warning-secret-sentinel")
            );
        }
        assert_eq!(
            std::fs::read_to_string(counter).unwrap(),
            "run\n",
            "replacement/reload reran context resolver"
        );
        assert!(
            changed.requests().is_empty(),
            "replacement/reload reread context endpoint"
        );
    }
}
