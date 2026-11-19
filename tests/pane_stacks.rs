//! Local patch: pane stacks over a real server and client socket (spec 002).

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
    cleanup_test_base, client_shell_handshake, register_runtime_dir,
    register_spawned_herdr_pid, send_endpoint_control, unregister_spawned_herdr_pid,
    CURRENT_ENDPOINT_PROTOCOL_GENERATION as CURRENT_PROTOCOL,
};

const STACK_CONTROL: &str = "endpoint.pane-stack.control.v1";

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
            "/tmp/herdr-stacks-{name}-{}-{nanos}",
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
        json!({"id": "stacks", "method": method, "params": params})
    )
    .unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    serde_json::from_str(&response).expect("valid JSON response")
}


const CONFIG: &str = "onboarding = false\n[terminal]\ndefault_shell = \"/bin/sh\"\n";

fn connect_client(dirs: &Dirs) -> UnixStream {
    let mut client = UnixStream::connect(dirs.client_socket()).expect("client connects");
    client_shell_handshake(&mut client, CURRENT_PROTOCOL, 80, 24).expect("shell handshake");
    client
}

fn stack_control(client: &mut UnixStream, control: Value) {
    send_endpoint_control(client, STACK_CONTROL, &control.to_string()).expect("send stack control");
}

fn ok(response: Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn panes(dirs: &Dirs) -> Vec<Value> {
    ok(api(&dirs.api_socket(), "pane.list", json!({})))["panes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn pane_text(dirs: &Dirs, pane_id: &str) -> String {
    ok(api(
        &dirs.api_socket(),
        "pane.read",
        json!({"pane_id": pane_id, "source": "recent", "lines": 200}),
    ))["read"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn wait_until<F: FnMut() -> bool>(what: &str, mut check: F) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

/// Create a workspace and stack a second shell on its root pane through the
/// client control. Returns (hidden root pane, visible new pane).
fn build_stack(dirs: &Dirs, client: &mut UnixStream) -> (String, String) {
    let created = ok(api(
        &dirs.api_socket(),
        "workspace.create",
        json!({"cwd": "/tmp", "focus": true}),
    ));
    let root = created["root_pane"]["pane_id"].as_str().unwrap().to_string();
    stack_control(client, json!({"op": "new", "pane_id": root}));
    let mut visible = String::new();
    wait_until("a stacked pane", || {
        let listed = panes(dirs);
        if let Some(pane) = listed.iter().find(|pane| {
            pane["stack"]["visible"] == true && pane["workspace_id"] == created["workspace"]["workspace_id"]
        }) {
            visible = pane["pane_id"].as_str().unwrap().to_string();
            true
        } else {
            false
        }
    });
    (root, visible)
}

fn send(dirs: &Dirs, pane_id: &str, text: &str) {
    ok(api(
        &dirs.api_socket(),
        "pane.send_input",
        json!({"pane_id": pane_id, "text": text, "keys": ["Enter"]}),
    ));
}

#[test]
fn stacked_panes_keep_their_own_output_and_are_listed_and_driven_while_hidden() {
    let _lock = test_lock();
    let dirs = Dirs::new("io", CONFIG);
    let _server = spawn_server(&dirs);
    let mut client = connect_client(&dirs);
    let (hidden, visible) = build_stack(&dirs, &mut client);

    // `pane run` reaches a hidden pane: input goes to it while it stays hidden.
    send(&dirs, &hidden, "echo HIDDEN-$((40+2))");
    send(&dirs, &visible, "echo SHOWN-$((40+3))");
    wait_until("hidden output", || pane_text(&dirs, &hidden).contains("HIDDEN-42"));
    wait_until("visible output", || pane_text(&dirs, &visible).contains("SHOWN-43"));
    assert!(!pane_text(&dirs, &visible).contains("HIDDEN-42"));
    assert!(!pane_text(&dirs, &hidden).contains("SHOWN-43"));

    let listed = panes(&dirs);
    let find = |id: &str| listed.iter().find(|pane| pane["pane_id"] == id).unwrap().clone();
    assert_eq!(find(&hidden)["stack"]["visible"], false);
    assert_eq!(find(&visible)["stack"]["visible"], true);
    assert_eq!(find(&hidden)["stack"]["stack_id"], find(&visible)["stack"]["stack_id"]);

    stack_control(&mut client, json!({"op": "show", "pane_id": hidden}));
    wait_until("the switch", || {
        panes(&dirs)
            .iter()
            .any(|pane| pane["pane_id"] == hidden && pane["stack"]["visible"] == true)
    });
    assert!(pane_text(&dirs, &hidden).contains("HIDDEN-42"), "output survives the switch");
}

#[test]
fn a_stack_survives_a_server_restart_with_the_same_visible_pane() {
    let _lock = test_lock();
    let dirs = Dirs::new("restart", CONFIG);
    {
        let _server = spawn_server(&dirs);
        let mut client = connect_client(&dirs);
        let (hidden, visible) = build_stack(&dirs, &mut client);
        for (pane, label) in [(&hidden, "alpha"), (&visible, "beta")] {
            ok(api(
                &dirs.api_socket(),
                "pane.rename",
                json!({"pane_id": pane, "label": label}),
            ));
        }
        ok(api(&dirs.api_socket(), "server.stop", json!({})));
        thread::sleep(Duration::from_millis(500));
    }
    let _server = spawn_server(&dirs);
    let listed = panes(&dirs);
    let by_label = |label: &str| {
        listed
            .iter()
            .find(|pane| pane["label"] == label)
            .unwrap_or_else(|| panic!("{label} restored: {listed:?}"))
            .clone()
    };
    assert_eq!(by_label("beta")["stack"]["visible"], true);
    assert_eq!(by_label("alpha")["stack"]["visible"], false);
    assert_eq!(by_label("alpha")["stack"]["stack_id"], by_label("beta")["stack"]["stack_id"]);
}

#[test]
fn a_live_handoff_carries_a_stack_and_its_hidden_process() {
    let _lock = test_lock();
    let dirs = Dirs::new("handoff", CONFIG);
    let server = spawn_server(&dirs);
    let mut client = connect_client(&dirs);
    let (hidden, visible) = build_stack(&dirs, &mut client);
    let marker = dirs.base.join("hidden.pid");
    send(
        &dirs,
        &hidden,
        &format!("sh -c 'echo $$ > {}; while read l; do echo got:$l; done'", marker.display()),
    );
    wait_until("the hidden child", || {
        fs::read_to_string(&marker).is_ok_and(|text| text.trim().parse::<i32>().is_ok())
    });
    let pid: i32 = fs::read_to_string(&marker).unwrap().trim().parse().unwrap();

    ok(api(&dirs.api_socket(), "server.live_handoff", json!({})));
    drop(client);
    drop(server);
    thread::sleep(Duration::from_millis(300));
    wait_for_socket(&dirs.api_socket());
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the hidden process survived");

    let listed = panes(&dirs);
    let stacked: Vec<_> = listed.iter().filter(|pane| !pane["stack"].is_null()).collect();
    assert_eq!(stacked.len(), 2, "both members came across: {listed:?}");
    let visible_after = stacked
        .iter()
        .find(|pane| pane["stack"]["visible"] == true)
        .unwrap();
    assert_eq!(visible_after["pane_id"], visible, "the same pane is shown");
    send(&dirs, &hidden, "after");
    wait_until("hidden input after handoff", || pane_text(&dirs, &hidden).contains("got:after"));
}
