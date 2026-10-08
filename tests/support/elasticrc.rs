use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const QUERY: &str = "FROM logs-* | KEEP message";
pub const ROWS: &str =
    r#"{"columns":[{"name":"message","type":"keyword"}],"values":[["context-row"]]}"#;
pub const MAPPING: &str =
    r#"{"logs-a":{"mappings":{"properties":{"message":{"type":"keyword"}}}}}"#;
pub const FIELD_CAPS: &str =
    r#"{"fields":{"message":{"keyword":{"searchable":true,"aggregatable":true}}}}"#;
pub const DISCOVERY: &str =
    r#"{"indices":[{"name":"logs-a","attributes":["open"]}],"aliases":[],"data_streams":[]}"#;
pub const TRANSPORT_ENV: [&str; 16] = [
    "ELASTIC_API_KEY",
    "ELASTIC_USERNAME",
    "ELASTIC_PASSWORD",
    "ELASTIC_CA_CERT",
    "ELASTIC_CLI_CONFIG_FILE",
    "TVIEW_CONTEXT_SECRET",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub config: PathBuf,
    pub home: PathBuf,
    pub xdg: PathBuf,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("context fixture");
        let home = root.path().join("home");
        let xdg = root.path().join("config");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg).unwrap();
        let config = root.path().join("selected.json");
        Self {
            root,
            config,
            home,
            xdg,
        }
    }

    pub fn write(&self, current: &str, contexts: Value) {
        Self::write_at(&self.config, current, contexts);
    }

    pub fn write_at(path: &Path, current: &str, contexts: Value) {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({
                "current_context": current, "contexts": contexts,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tview"));
        command.current_dir(self.root.path());
        for name in TRANSPORT_ENV {
            command.env_remove(name);
        }
        command
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("ELASTIC_CLI_CONFIG_FILE", &self.config)
            .args(["--color", "never"]);
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("run tview")
    }

    pub fn view(&self, name: &str, contents: &str) -> PathBuf {
        let views = self.xdg.join("tview/views");
        std::fs::create_dir_all(&views).unwrap();
        let path = views.join(format!("{name}.yml"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[cfg(unix)]
    pub fn resolver(&self, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = self.root.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        format!("$(cmd:{})", shell_quote(&path))
    }

    #[cfg(unix)]
    pub fn shell_environment(&self) -> String {
        let removals = TRANSPORT_ENV
            .iter()
            .map(|name| format!("-u {name}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("env {removals} TERM=xterm-256color HOME={} USERPROFILE={} XDG_CONFIG_HOME={} ELASTIC_CLI_CONFIG_FILE={}",
            shell_quote(&self.home), shell_quote(&self.home), shell_quote(&self.xdg), shell_quote(&self.config))
    }
}

pub fn service(endpoint: &str, auth: Option<Value>) -> Value {
    let mut service = json!({ "url": endpoint });
    if let Some(auth) = auth {
        service["auth"] = auth;
    }
    json!({ "elasticsearch": service })
}

pub fn success(output: &Output) {
    assert!(
        output.status.success(),
        "status {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn preparation_failure(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "preparation emitted stdout: {:?}",
        output.stdout
    );
    assert!(!output.stderr.is_empty(), "missing preparation diagnostic");
}

pub fn no_secrets(output: &Output, secrets: &[&str]) {
    for secret in secrets {
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains(secret),
            "secret in stdout"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains(secret),
            "secret in stderr"
        );
    }
}

pub fn authorization(request: &str) -> Option<&str> {
    request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim())
    })
}

pub fn request_query(request: &str) -> String {
    let body = request.split_once("\r\n\r\n").expect("HTTP body").1;
    serde_json::from_str::<Value>(body).unwrap()["query"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[cfg(unix)]
pub struct TlsServer {
    pub endpoint: String,
    pub ca: PathBuf,
    child: std::process::Child,
    _root: tempfile::TempDir,
}

#[cfg(unix)]
impl TlsServer {
    pub fn start() -> Self {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        let root = tempfile::tempdir().unwrap();
        let ca = root.path().join("ca.pem");
        let ca_key = root.path().join("ca.key");
        let certificate = root.path().join("localhost.pem");
        let key = root.path().join("localhost.key");
        let request = root.path().join("localhost.csr");
        let ca_config = root.path().join("ca.cnf");
        let leaf_config = root.path().join("localhost.cnf");
        // Use a dedicated signing CA and a server-authentication leaf; trusting
        // a CA certificate as the server leaf is rejected by macOS TLS policy.
        std::fs::write(&ca_config, "[req]\ndistinguished_name=dn\nx509_extensions=ca\nprompt=no\n[dn]\nCN=Tview Context Test CA\n[ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n").unwrap();
        std::fs::write(&leaf_config, "[req]\ndistinguished_name=dn\nreq_extensions=server\nprompt=no\n[dn]\nCN=localhost\n[server]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectKeyIdentifier=hash\n").unwrap();
        let generated_ca = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256", "-days", "1", "-config",
            ])
            .arg(&ca_config)
            .arg("-keyout")
            .arg(&ca_key)
            .arg("-out")
            .arg(&ca)
            .output()
            .expect("openssl TLS fixture prerequisite");
        assert!(
            generated_ca.status.success(),
            "openssl CA: {}",
            String::from_utf8_lossy(&generated_ca.stderr)
        );
        let generated_request = Command::new("openssl")
            .args([
                "req", "-new", "-newkey", "rsa:2048", "-nodes", "-sha256", "-config",
            ])
            .arg(&leaf_config)
            .arg("-keyout")
            .arg(&key)
            .arg("-out")
            .arg(&request)
            .output()
            .expect("generate localhost certificate request");
        assert!(
            generated_request.status.success(),
            "openssl request: {}",
            String::from_utf8_lossy(&generated_request.stderr)
        );
        let signed_leaf = Command::new("openssl")
            .args(["x509", "-req", "-sha256", "-days", "1", "-in"])
            .arg(&request)
            .arg("-CA")
            .arg(&ca)
            .arg("-CAkey")
            .arg(&ca_key)
            .arg("-CAcreateserial")
            .arg("-extfile")
            .arg(&leaf_config)
            .args(["-extensions", "server"])
            .arg("-out")
            .arg(&certificate)
            .output()
            .expect("sign localhost server certificate");
        assert!(
            signed_leaf.status.success(),
            "openssl leaf: {}",
            String::from_utf8_lossy(&signed_leaf.stderr)
        );
        let script = r#"
import ssl, socket, sys
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(sys.argv[1], sys.argv[2])
listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen()
print(listener.getsockname()[1], flush=True)
body = sys.argv[3].encode()
while True:
    connection, _ = listener.accept()
    try:
        with context.wrap_socket(connection, server_side=True) as stream:
            stream.settimeout(3)
            request = b''
            while b'\r\n\r\n' not in request:
                chunk = stream.recv(4096)
                if not chunk: break
                request += chunk
            header, _, pending = request.partition(b'\r\n\r\n')
            length = next((int(line.split(b':', 1)[1]) for line in header.split(b'\r\n') if line.lower().startswith(b'content-length:')), 0)
            while len(pending) < length:
                chunk = stream.recv(4096)
                if not chunk: break
                pending += chunk
            stream.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
    except (ssl.SSLError, OSError):
        connection.close()
"#;
        let mut child = Command::new("python3")
            .args(["-u", "-c", script])
            .arg(&certificate)
            .arg(&key)
            .arg(ROWS)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 TLS fixture prerequisite");
        let mut port = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let port: u16 = port.trim().parse().expect("TLS listener ready");
        Self {
            endpoint: format!("https://localhost:{port}"),
            ca,
            child,
            _root: root,
        }
    }
}

#[cfg(unix)]
impl Drop for TlsServer {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}
