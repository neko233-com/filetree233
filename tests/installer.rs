use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
struct Server {
    base: String,
    state: Arc<AtomicU8>,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(binary: Vec<u8>, name: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(AtomicU8::new(0));
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        let mode = state.clone();
        let hash = format!("{:x}", Sha256::digest(&binary));
        let handle = thread::spawn(move || {
            while flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                        let mut request_bytes = Vec::new();
                        let mut buf = [0u8; 1024];
                        while request_bytes.len() < 16 * 1024
                            && !request_bytes.ends_with(b"\r\n\r\n")
                        {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(count) => request_bytes.extend_from_slice(&buf[..count]),
                            }
                        }
                        if !request_bytes.ends_with(b"\r\n\r\n") {
                            continue;
                        }
                        let request = String::from_utf8_lossy(&request_bytes);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("");
                        let expected_file = format!("/v0.1.0-alpha.1/{name}");
                        if path == expected_file {
                            if mode
                                .compare_exchange(3, 0, Ordering::Relaxed, Ordering::Relaxed)
                                .is_ok()
                            {
                                let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            } else {
                                respond(&mut stream, &binary);
                            }
                        } else if path == "/v0.1.0-alpha.1/SHA256SUMS" {
                            let row = format!(
                                "{}  {}\n",
                                if mode.load(Ordering::Relaxed) == 1 {
                                    "0".repeat(64)
                                } else {
                                    hash.clone()
                                },
                                name
                            );
                            let body = if mode.load(Ordering::Relaxed) == 2 {
                                format!("{row}{row}")
                            } else {
                                row
                            };
                            respond(&mut stream, body.as_bytes());
                        } else {
                            let _=stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("server accept: {e}"),
                }
            }
        });
        Self {
            base,
            state,
            running,
            handle: Some(handle),
        }
    }
}
fn respond(stream: &mut TcpStream, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .expect("write HTTP headers");
    stream.write_all(body).expect("write HTTP body");
    stream.flush().expect("flush HTTP response");
}
impl Drop for Server {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "filetree233-install-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn asset_name() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "filetree233-windows-x64.exe",
        ("windows", "aarch64") => "filetree233-windows-arm64.exe",
        ("linux", "x86_64") => "filetree233-linux-x64",
        ("linux", "aarch64") => "filetree233-linux-arm64",
        ("macos", "x86_64") => "filetree233-macos-x64",
        ("macos", "aarch64") => "filetree233-macos-arm64",
        _ => panic!("unsupported test platform"),
    }
}
fn install(base: &str, dir: &Path) -> std::process::Output {
    #[cfg(windows)]
    let mut cmd = {
        let mut command = Command::new("pwsh");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-File",
                "scripts/install.ps1",
                "-Version",
                "v0.1.0-alpha.1",
                "-InstallDir",
            ])
            .arg(dir);
        command
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut command = Command::new("sh");
        command
            .arg("scripts/install.sh")
            .env("FILETREE233_VERSION", "v0.1.0-alpha.1")
            .env("FILETREE233_INSTALL_DIR", dir);
        command
    };
    cmd.env("FILETREE233_RELEASE_BASE", base)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap()
}
#[cfg(windows)]
fn install_legacy_powershell(base: &str, dir: &Path) -> std::process::Output {
    Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-File",
            "scripts/install.ps1",
            "-Version",
            "v0.1.0-alpha.1",
            "-InstallDir",
        ])
        .arg(dir)
        .env("FILETREE233_RELEASE_BASE", base)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap()
}
#[test]
#[ignore = "run against the release executable in the installer workflow"]
fn installers_download_verify_install_and_preserve_on_bad_checksum() {
    let fixture = Fixture::new();
    let binary = std::env::var_os("FILETREE233_INSTALL_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_filetree233")));
    let bytes = fs::read(&binary).unwrap();
    let digest = Sha256::digest(&bytes);
    let server = Server::start(bytes, asset_name().to_string());
    let destination = fixture.0.join("bin with spaces");
    let output = install(&server.base, &destination);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let installed = destination.join(if cfg!(windows) {
        "filetree233.exe"
    } else {
        "filetree233"
    });
    assert_eq!(Sha256::digest(fs::read(&installed).unwrap()), digest);
    let result = Command::new(&installed).arg("--version").output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        format!("filetree233 {}", env!("CARGO_PKG_VERSION"))
    );
    server.state.store(3, Ordering::Relaxed);
    let replacement = install(&server.base, &destination);
    assert!(
        replacement.status.success(),
        "replacement failed: {}",
        String::from_utf8_lossy(&replacement.stderr)
    );
    assert_eq!(Sha256::digest(fs::read(&installed).unwrap()), digest);
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        1,
        "staged file left behind"
    );
    assert_eq!(
        server.state.load(Ordering::Relaxed),
        0,
        "transient failure was not exercised"
    );
    for mode in [1, 2] {
        server.state.store(mode, Ordering::Relaxed);
        let output = install(&server.base, &destination);
        assert!(
            !output.status.success(),
            "invalid checksum unexpectedly installed"
        );
        assert_eq!(Sha256::digest(fs::read(&installed).unwrap()), digest);
    }
    let invalid_base = install("http://127.0.0.1:123@invalid.example", &destination);
    assert!(!invalid_base.status.success());
    assert_eq!(Sha256::digest(fs::read(&installed).unwrap()), digest);
    #[cfg(windows)]
    {
        server.state.store(0, Ordering::Relaxed);
        let legacy_dir = fixture.0.join("windows powershell 5");
        let legacy = install_legacy_powershell(&server.base, &legacy_dir);
        assert!(
            legacy.status.success(),
            "Windows PowerShell failed: {}",
            String::from_utf8_lossy(&legacy.stderr)
        );
        let legacy_exe = legacy_dir.join("filetree233.exe");
        assert_eq!(Sha256::digest(fs::read(&legacy_exe).unwrap()), digest);
        assert!(
            Command::new(legacy_exe)
                .arg("--version")
                .output()
                .unwrap()
                .status
                .success()
        );
        let legacy_replacement = install_legacy_powershell(&server.base, &legacy_dir);
        assert!(
            legacy_replacement.status.success(),
            "Windows PowerShell replacement failed: {}",
            String::from_utf8_lossy(&legacy_replacement.stderr)
        );
        assert_eq!(fs::read_dir(&legacy_dir).unwrap().count(), 1);
    }
}
