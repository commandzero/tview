#![cfg(unix)]
// PTY process setup and signal delivery require libc; production code stays denied.
#![allow(unsafe_code)]

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

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
