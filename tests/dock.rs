//! Local patch: the dock over a real server and client socket (spec 001).

#![cfg(unix)]

pub mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use support::{
    cleanup_test_base, client_shell_handshake, read_endpoint_control, register_runtime_dir,
    register_spawned_herdr_pid, send_endpoint_control, unregister_spawned_herdr_pid,
    CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL,
};

const DOCK_SURFACE: &str = "endpoint.dock.surface.v1";
const DOCK_CONTROL: &str = "endpoint.dock.control.v1";

struct Server {
    _master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        support::stop_spawned_herdr(&mut *self.child);
        drop(self._master.take());
        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                let mut status = 0;
                let result =
                    unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                if result == pid as libc::pid_t || result == -1 {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            unregister_spawned_herdr_pid(Some(pid));
        }
    }
}

struct Dirs {
    base: PathBuf,
    config_home: PathBuf,
    runtime_dir: PathBuf,
}

impl Dirs {
    fn new(name: &str, config: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = PathBuf::from(format!(
            "/tmp/herdr-dock-{name}-{}-{nanos}",
            std::process::id()
        ));
        let config_home = base.join("config");
        let runtime_dir = base.join("runtime");
        fs::create_dir_all(config_home.join("herdr")).unwrap();
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::write(config_home.join("herdr/config.toml"), config).unwrap();
        Self {
            base,
            config_home,
            runtime_dir,
        }
    }

    fn api_socket(&self) -> PathBuf {
        self.runtime_dir.join("herdr.sock")
    }

    fn client_socket(&self) -> PathBuf {
        self.runtime_dir.join("herdr-client.sock")
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        cleanup_test_base(&self.base);
    }
}

fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait_for_socket(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("socket did not appear at {}", path.display());
}

fn spawn_server(dirs: &Dirs) -> Server {
    register_runtime_dir(&dirs.runtime_dir);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_herdr"));
    support::isolate_herdr_test_process(&mut cmd);
    cmd.arg("server");
    cmd.env("XDG_STATE_HOME", dirs.runtime_dir.join("state"));
    cmd.env("XDG_CONFIG_HOME", &dirs.config_home);
    cmd.env("XDG_RUNTIME_DIR", &dirs.runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", dirs.api_socket());
    // Debug builds read `herdr-dev/`; name the file so either build finds it.
    cmd.env(
        "HERDR_CONFIG_PATH",
        dirs.config_home.join("herdr/config.toml"),
    );
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("HERDR_ENV");
    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_herdr_pid(child.process_id());
    drop(pair.slave);
    wait_for_socket(&dirs.api_socket());
    wait_for_socket(&dirs.client_socket());
    Server {
        _master: Some(pair.master),
        child,
    }
}

fn api(socket: &Path, method: &str, params: Value) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connect to the API socket");
    writeln!(
        stream,
        "{}",
        json!({"id": "dock", "method": method, "params": params})
    )
    .unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    serde_json::from_str(&response).expect("valid JSON response")
}

/// Connect a client shell and opt in to the dock with its width.
fn connect_docked(dirs: &Dirs) -> UnixStream {
    let mut client = UnixStream::connect(dirs.client_socket()).expect("client connects");
    client_shell_handshake(&mut client, CURRENT_PROTOCOL, 80, 24).expect("shell handshake");
    control(&mut client, json!({"op": "width", "cols": 80}));
    client
}

fn control(client: &mut UnixStream, control: Value) {
    send_endpoint_control(client, DOCK_CONTROL, &control.to_string()).expect("send dock control");
}

fn frame_text(surface: &Value) -> String {
    surface["frame"]["cells"]
        .as_array()
        .map(|cells| {
            cells
                .iter()
                .map(|cell| cell["symbol"].as_str().unwrap_or(" "))
                .collect()
        })
        .unwrap_or_default()
}

/// The next dock surface that satisfies `accept`.
fn wait_for_dock<F>(client: &mut UnixStream, mut accept: F) -> Value
where
    F: FnMut(&Value) -> bool,
{
    let data = read_endpoint_control(client, DOCK_SURFACE, Duration::from_secs(10), |data| {
        serde_json::from_str::<Value>(data)
            .map(|surface| accept(&surface))
            .unwrap_or(false)
    })
    .expect("a matching dock surface");
    serde_json::from_str(&data).unwrap()
}

const BASE_CONFIG: &str = "onboarding = false\n[terminal]\ndefault_shell = \"/bin/sh\"\n";

#[test]
fn dock_output_reaches_the_client_and_dock_keys_reach_only_the_dock() {
    let _lock = test_lock();
    let dirs = Dirs::new(
        "io",
        &format!(
            "{BASE_CONFIG}[[ui.dock.panes]]\nlabel = \"probe\"\ncommand = \"printf 'DOCK-READY\\\\n'; exec cat\"\n"
        ),
    );
    let _server = spawn_server(&dirs);
    let mut client = connect_docked(&dirs);

    let surface = wait_for_dock(&mut client, |surface| {
        frame_text(surface).contains("DOCK-READY")
    });
    assert_eq!(surface["rows"], 3);
    assert_eq!(surface["cols"], 80);
    let pane_id = surface["panes"][0]["pane_id"].as_str().unwrap().to_owned();
    assert_eq!(surface["panes"][0]["label"], "probe");

    control(
        &mut client,
        json!({"op": "input", "pane_id": pane_id, "events": [{"Paste": "hello-dock"}]}),
    );
    wait_for_dock(&mut client, |surface| {
        frame_text(surface).contains("hello-dock")
    });

    let panes = api(&dirs.api_socket(), "pane.list", json!({}));
    let workspace_panes = panes["result"]["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        workspace_panes.iter().all(|pane| !pane["pane_id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("dock_")),
        "dock panes are not API panes: {panes}"
    );
    for pane in workspace_panes {
        let read = api(
            &dirs.api_socket(),
            "pane.read",
            json!({"pane_id": pane["pane_id"], "source": "recent", "lines": 200}),
        );
        let text = read["result"]["read"]["text"].as_str().unwrap_or_default();
        assert!(
            !text.contains("hello-dock"),
            "a workspace pane got dock input: {text}"
        );
    }
}

#[test]
fn a_split_dock_survives_a_server_restart() {
    let _lock = test_lock();
    let dirs = Dirs::new(
        "restart",
        &format!("{BASE_CONFIG}[[ui.dock.panes]]\nlabel = \"probe\"\ncommand = \"exec cat\"\n"),
    );
    let before = {
        let _server = spawn_server(&dirs);
        let mut client = connect_docked(&dirs);
        let surface = wait_for_dock(&mut client, |surface| surface["rows"] == 3);
        let pane_id = surface["panes"][0]["pane_id"].as_str().unwrap().to_owned();
        control(
            &mut client,
            json!({"op": "split", "pane_id": pane_id, "right": false}),
        );
        control(&mut client, json!({"op": "rows", "rows": 7}));
        control(
            &mut client,
            json!({"op": "ratio", "path": [], "ratio": 0.3}),
        );
        wait_for_dock(&mut client, |surface| {
            surface["rows"] == 7
                && surface["panes"]
                    .as_array()
                    .is_some_and(|panes| panes.len() == 2)
                && surface["splits"][0]["pos"]
                    .as_u64()
                    .is_some_and(|pos| pos < 3)
        })
    };

    let _server = spawn_server(&dirs);
    let mut client = connect_docked(&dirs);
    let after = wait_for_dock(&mut client, |surface| surface["rows"] != 0);
    assert_eq!(after["rows"], 7, "the shared height is restored");
    assert_eq!(after["panes"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        after["splits"][0]["pos"], before["splits"][0]["pos"],
        "the ratio is restored"
    );
    assert_eq!(after["splits"][0]["vertical_divider"], false);
}
