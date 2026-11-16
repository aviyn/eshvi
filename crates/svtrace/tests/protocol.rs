//! The language server over an in-memory connection, with a stand-in for slang that prints a
//! checked-in dump, and the real slang when it is installed.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use common::{data_dir, DRIVERS, LOADS};
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{json, Value};

/// A workspace folder holding cone_test.sv and a `.slang/server.json` that names a build file.
fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("svtrace-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".slang")).unwrap();
    fs::copy(data_dir().join("cone_test.sv"), dir.join("cone_test.sv")).unwrap();
    fs::write(dir.join("cone_test.f"), "cone_test.sv\n").unwrap();
    fs::write(dir.join(".slang/server.json"), r#"{ "build": "cone_test.f" }"#).unwrap();
    // The dir is canonical so that URIs can be compared with the ones svtrace makes.
    fs::canonicalize(dir).unwrap()
}

/// A program that prints cone_test's dump and exits with status 1, as slang does for a design with
/// errors, and counts its runs.
fn fake_slang(dir: &Path) -> PathBuf {
    let script = dir.join("fake-slang");
    let runs = dir.join("runs");
    let dump = data_dir().join("cone_test.json");
    fs::write(&script, format!("#!/bin/sh\necho run >> '{}'\ncat '{}'\nexit 1\n", runs.display(), dump.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}

struct Client {
    connection: Connection,
    next_id: i32,
    /// Replies that came while another was waited for.
    early: Vec<lsp_server::Response>,
    /// What the server asked and told the client, as it came while a reply was waited for.
    asked: Vec<Request>,
    told: Vec<Notification>,
}

impl Client {
    fn send(&mut self, method: &str, params: Value) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.connection.sender.send(Message::Request(Request::new(id.clone(), method.to_string(), params))).unwrap();
        id
    }

    /// The reply to a request, which may already have come.
    fn reply(&mut self, id: &RequestId) -> Value {
        loop {
            let response = match self.early.iter().position(|response| &response.id == id) {
                Some(at) => self.early.remove(at),
                None => match self.connection.receiver.recv_timeout(Duration::from_secs(30)).expect("a reply") {
                    Message::Response(response) => response,
                    Message::Request(request) => {
                        self.asked.push(request);
                        continue;
                    }
                    Message::Notification(notification) => {
                        self.told.push(notification);
                        continue;
                    }
                },
            };
            if &response.id != id {
                self.early.push(response);
                continue;
            }
            return match response.error {
                Some(error) => json!({ "error": { "code": error.code, "message": error.message } }),
                None => response.result.unwrap_or(Value::Null),
            };
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.reply(&id)
    }

    fn notify(&self, method: &str, params: Value) {
        self.connection.sender.send(Message::Notification(Notification::new(method.to_string(), params))).unwrap();
    }

    fn command(&mut self, command: &str, arguments: Value) -> Value {
        self.request("workspace/executeCommand", json!({ "command": command, "arguments": arguments }))
    }
}

fn start(dir: &Path, slang: PathBuf) -> (Client, thread::JoinHandle<anyhow::Result<()>>) {
    start_with(dir, slang, Duration::from_secs(120))
}

fn start_with(dir: &Path, slang: PathBuf, timeout: Duration) -> (Client, thread::JoinHandle<anyhow::Result<()>>) {
    start_as(dir, slang, timeout, json!({}))
}

/// [`start_with`], as a client with these capabilities.
fn start_as(dir: &Path, slang: PathBuf, timeout: Duration, capabilities: Value) -> (Client, thread::JoinHandle<anyhow::Result<()>>) {
    let (client, server) = Connection::memory();
    let handle = thread::spawn(move || svtrace::lsp::serve_with(server, slang, timeout));
    let mut client = Client { connection: client, next_id: 0, early: Vec::new(), asked: Vec::new(), told: Vec::new() };
    let uri = format!("file://{}", dir.display());
    let result = client.request("initialize", json!({ "processId": null, "rootUri": uri, "capabilities": capabilities }));
    assert_eq!(result["serverInfo"]["name"], "svtrace");
    assert_eq!(result["capabilities"]["textDocumentSync"], json!({ "openClose": false, "change": 0, "save": { "includeText": false } }));
    assert_eq!(result["capabilities"]["experimental"]["svtrace"]["paths"], json!(true));
    let commands = result["capabilities"]["executeCommandProvider"]["commands"].as_array().unwrap();
    for name in [DRIVERS, LOADS, "slang.getDriversWithConstants", "svtrace.step", "svtrace.endpoints", "svtrace.reload", "svtrace.status", "svtrace.listSignals", "svtrace.useBuild"] {
        assert!(commands.contains(&json!(name)), "{name} in {commands:?}");
    }
    client.notify("initialized", json!({}));
    (client, handle)
}

fn finish(mut client: Client, handle: thread::JoinHandle<anyhow::Result<()>>) {
    assert_eq!(client.request("shutdown", Value::Null), Value::Null);
    client.notify("exit", Value::Null);
    handle.join().unwrap().unwrap();
}

fn runs(dir: &Path) -> usize {
    fs::read_to_string(dir.join("runs")).map_or(0, |runs| runs.lines().count())
}

/// A stand-in for slang that records its process id and never finishes (`exec`, so that killing it
/// kills the sleep).
fn hanging_slang(dir: &Path) -> PathBuf {
    let script = dir.join("hanging-slang");
    fs::write(&script, format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 600\n", dir.join("pid").display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn alive(dir: &Path) -> bool {
    let pid = fs::read_to_string(dir.join("pid")).unwrap();
    std::process::Command::new("kill").args(["-0", pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap().success()
}

fn wait_for_pid(dir: &Path) {
    let started = Instant::now();
    while !dir.join("pid").exists() {
        assert!(started.elapsed() < Duration::from_secs(10), "slang never started");
        thread::sleep(Duration::from_millis(20));
    }
}

fn joins_within(handle: thread::JoinHandle<anyhow::Result<()>>, seconds: u64) {
    let started = Instant::now();
    while !handle.is_finished() {
        assert!(started.elapsed() < Duration::from_secs(seconds), "the server is still running {seconds} s later");
        thread::sleep(Duration::from_millis(20));
    }
    handle.join().unwrap().unwrap();
}

#[test]
fn shutdown_and_exit_are_prompt_while_slang_hangs_and_kill_it() {
    let dir = workspace("hang-exit");
    let (mut client, handle) = start(&dir, hanging_slang(&dir));
    wait_for_pid(&dir);
    // A question waiting for the first build does not hold up `shutdown`.
    let waiting = client.send("workspace/executeCommand", json!({ "command": "slang.getScopes", "arguments": [] }));
    let started = Instant::now();
    assert_eq!(client.request("shutdown", Value::Null), Value::Null);
    client.notify("exit", Value::Null);
    joins_within(handle, 5);
    assert!(started.elapsed() < Duration::from_secs(5));
    // (Its question gets an answer or none, not a hang.)
    let _ = waiting;
    let killed = Instant::now();
    while alive(&dir) {
        assert!(killed.elapsed() < Duration::from_secs(5), "slang was left running");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_closed_connection_ends_the_server_and_kills_slang() {
    let dir = workspace("hang-close");
    let (client, handle) = start(&dir, hanging_slang(&dir));
    wait_for_pid(&dir);
    drop(client);
    joins_within(handle, 5);
    let killed = Instant::now();
    while alive(&dir) {
        assert!(killed.elapsed() < Duration::from_secs(5), "slang was left running");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_question_during_a_hung_first_build_gets_an_error_after_the_limit() {
    let dir = workspace("hang-first");
    let (mut client, handle) = start_with(&dir, hanging_slang(&dir), Duration::from_secs(1));
    let started = Instant::now();
    let answer = client.command("slang.getScopes", json!([]));
    let message = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("svtrace: no design: slang did not finish in 1 s"), "{answer}");
    assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
    assert_eq!(client.command("svtrace.status", json!([]))["state"], "failed");
    assert!(!alive(&dir), "slang was left running after the limit");
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn other_requests_are_answered_while_a_reload_runs() {
    let dir = workspace("reload-hang");
    // The first run prints the dump; later ones hang.
    let script = dir.join("slow-second-slang");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho run >> '{runs}'\nif [ \"$(wc -l < '{runs}')\" -ge 2 ]; then exec sleep 600; fi\ncat '{dump}'\nexit 1\n",
            runs = dir.join("runs").display(),
            dump = data_dir().join("cone_test.json").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let (mut client, handle) = start_with(&dir, script, Duration::from_secs(3));
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);

    let reload = client.send("workspace/executeCommand", json!({ "command": "svtrace.reload", "arguments": [] }));
    let started = Instant::now();
    // The old graph answers, and the status says what is going on, while the reload waits.
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);
    assert!(client.command("svtrace.listSignals", json!([])).as_array().is_some());
    assert_eq!(client.command("svtrace.status", json!([]))["state"], "building");
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());

    // The reload's answer comes when slang is given up on; the old design stays.
    let status = client.reply(&reload);
    assert_eq!(status["state"], "failed");
    assert!(status["error"].as_str().unwrap().starts_with("slang did not finish in 3 s"), "{status}");
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

/// The workspace reached through a symbolic link, as a buffer opened through `/tmp` is on macOS.
fn linked(dir: &Path) -> PathBuf {
    let link = dir.parent().unwrap().join(format!("svtrace-link-{}", std::process::id()));
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(dir, &link).unwrap();
    link
}

#[test]
fn at_cursor_and_saves_find_files_reached_through_a_symbolic_link() {
    let dir = workspace("link");
    let link = linked(&dir);
    let (mut client, handle) = start(&dir, fake_slang(&dir));
    let at = |uri: String| json!([{ "textDocument": { "uri": uri }, "position": { "line": 23, "character": 17 } }]);
    let direct = client.command("svtrace.atCursor", at(format!("file://{}/cone_test.sv", dir.display())));
    assert_eq!(direct, json!([{ "path": "test.x1", "signal": "test.x1" }]));
    let through_link = client.command("svtrace.atCursor", at(format!("file://{}/cone_test.sv", link.display())));
    assert_eq!(through_link, direct);

    // A save of a source file of the workspace, named through the link, rebuilds.
    client.command("svtrace.status", json!([]));
    assert_eq!(runs(&dir), 1);
    client.notify("textDocument/didSave", json!({ "textDocument": { "uri": format!("file://{}/new_file.sv", link.display()) } }));
    let started = Instant::now();
    while runs(&dir) < 2 {
        assert!(started.elapsed() < Duration::from_secs(10), "no rebuild after a save through a link");
        thread::sleep(Duration::from_millis(20));
    }
    finish(client, handle);
    let _ = fs::remove_file(link);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn server_json_env_does_not_reach_slang_unless_it_is_harmless() {
    let dir = workspace("env");
    fs::write(
        dir.join(".slang/server.json"),
        r#"{ "build": "cone_test.f", "env": { "LD_PRELOAD": "./x.so", "DYLD_INSERT_LIBRARIES": "./x.dylib", "PATH": "/nowhere", "SVTRACE_TEST_VAR": "kept" } }"#,
    )
    .unwrap();
    let script = dir.join("env-slang");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$LD_PRELOAD\" \"$DYLD_INSERT_LIBRARIES\" \"$SVTRACE_TEST_VAR\" > '{}'\ncase \"$PATH\" in /nowhere) echo path-changed >> '{}';; esac\ncat '{}'\nexit 1\n",
            dir.join("seen").display(),
            dir.join("seen").display(),
            data_dir().join("cone_test.json").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let (mut client, handle) = start(&dir, script);
    client.command(DRIVERS, json!(["test.x1"]));
    assert_eq!(client.command("svtrace.status", json!([]))["state"], "ready");
    assert_eq!(fs::read_to_string(dir.join("seen")).unwrap(), "||kept\n");
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn answers_the_commands_and_reports_errors() {
    let dir = workspace("answers");
    let (mut client, handle) = start(&dir, fake_slang(&dir));

    // The first question waits for the first build.
    let drivers = client.command(DRIVERS, json!(["test.the_sub_2.x"]));
    let paths: Vec<&str> = drivers.as_array().unwrap().iter().map(|entry| entry["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["test.the_sub_2.a", "test.the_sub_2.b", "test.the_sub_2.clk"]);
    let uri = drivers[0]["location"]["uri"].as_str().unwrap();
    assert_eq!(uri, format!("file://{}/cone_test.sv", dir.display()));

    let loads = client.command(LOADS, json!(["test.a"]));
    assert_eq!(loads.as_array().unwrap().len(), 2);
    let both = client.command("slang.getDriversWithConstants", json!(["test.the_sub_2.x"]));
    assert_eq!(both["drivers"], drivers);
    assert_eq!(both["constants"], json!([]));

    let error = client.command(DRIVERS, json!(["test.nope"]));
    assert_eq!(error["error"]["code"], -32603);
    assert_eq!(error["error"]["message"], "Could not find path in compiled design: test.nope");
    assert_eq!(client.command(DRIVERS, json!(["a", "b"]))["error"]["message"], "Expected 0 or 1 argument for command");
    assert!(client.command(DRIVERS, json!([]))["error"]["message"].as_str().is_some());
    // The whole-graph commands take an object.
    let step = client.command("svtrace.step", json!([{ "path": "test.the_sub_2.x", "direction": "drivers", "follow": false }]));
    let step_paths: Vec<&str> = step.as_array().unwrap().iter().map(|entry| entry["path"].as_str().unwrap()).collect();
    assert_eq!(step_paths, paths);
    assert_eq!(step[0]["sites"], drivers[0]["sites"]);
    let endpoints = client.command("svtrace.endpoints", json!([{ "path": "test.the_sub_2.x", "direction": "drivers" }]));
    assert!(!endpoints.as_array().unwrap().is_empty());
    assert_eq!(client.command("svtrace.step", json!([{ "path": "test.nope" }]))["error"]["message"], "Could not find path in compiled design: test.nope");
    assert!(client.command("svtrace.endpoints", json!(["test.x1"]))["error"]["message"].as_str().unwrap().contains("takes { path, direction }"));
    // A command that isn't svtrace's is an error, not an empty answer.
    assert_eq!(client.command("slang.getInstances", json!([]))["error"]["message"], "Unknown command: slang.getInstances");
    assert_eq!(client.command("svtrace.stepp", json!([{ "path": "test.x1" }]))["error"]["message"], "Unknown command: svtrace.stepp");
    // (This client didn't say it can watch files, so it isn't asked to.)
    assert!(client.asked.is_empty(), "{:?}", client.asked);

    let signals = client.command("svtrace.listSignals", json!([]));
    assert!(signals.as_array().unwrap().contains(&json!("test.the_sub_1.x")));
    let status = client.command("svtrace.status", json!([]));
    assert_eq!(status["state"], "ready");
    assert_eq!(status["slangExit"], 1);
    assert_eq!(status["signals"], signals.as_array().unwrap().len());
    assert!(status["builtAt"].as_u64().unwrap() > 0);
    assert!(status["stderrTail"].as_array().unwrap().is_empty());
    assert_eq!(runs(&dir), 1);

    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn reload_rebuilds_and_a_save_rebuilds_after_a_pause() {
    let dir = workspace("rebuild");
    let (mut client, handle) = start(&dir, fake_slang(&dir));
    client.command("svtrace.status", json!([]));
    client.command(DRIVERS, json!(["test.x1"]));
    assert_eq!(runs(&dir), 1);

    let status = client.command("svtrace.reload", json!([]));
    assert_eq!(status["state"], "ready");
    assert_eq!(runs(&dir), 2);

    // Saves close together build once; one of a file that isn't part of the design builds nothing.
    let save = |name: &str| client.notify("textDocument/didSave", json!({ "textDocument": { "uri": format!("file://{}/{name}", dir.display()) } }));
    save("cone_test.sv");
    save("cone_test.sv");
    save("cone_test.f");
    save("notes.txt");
    let started = Instant::now();
    while runs(&dir) < 3 {
        assert!(started.elapsed() < Duration::from_secs(10), "no rebuild after a save");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(600));
    assert_eq!(runs(&dir), 3);
    // (The old graph answered the whole time; the new one answers now.)
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);

    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_failed_build_is_reported_and_questions_get_its_error() {
    let dir = workspace("failed");
    fs::remove_file(dir.join(".slang/server.json")).unwrap();
    let (mut client, handle) = start(&dir, fake_slang(&dir));
    let answer = client.command(DRIVERS, json!(["test.x1"]));
    assert!(answer["error"]["message"].as_str().unwrap().starts_with("svtrace: no design: no build configured"), "{answer}");
    let status = client.command("svtrace.status", json!([]));
    assert_eq!(status["state"], "failed");
    assert!(status["error"].as_str().unwrap().starts_with("no build configured"));
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_file_changed_outside_the_editor_rebuilds_when_the_client_watches_files() {
    let dir = workspace("watched");
    let capabilities = json!({ "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } });
    let (mut client, handle) = start_as(&dir, fake_slang(&dir), Duration::from_secs(120), capabilities);
    assert_eq!(client.command("svtrace.status", json!([]))["state"], "building");
    client.command(DRIVERS, json!(["test.x1"]));
    assert_eq!(runs(&dir), 1);
    // The client was asked to watch the configuration, the build files and the sources of the workspace.
    let [registration] = client.asked.as_slice() else { panic!("{:?}", client.asked) };
    assert_eq!(registration.method, "client/registerCapability");
    let registered = &registration.params["registrations"][0];
    assert_eq!(registered["method"], "workspace/didChangeWatchedFiles");
    let watchers = registered["registerOptions"]["watchers"].as_array().unwrap();
    let patterns: Vec<&str> = watchers.iter().map(|watcher| watcher["globPattern"]["pattern"].as_str().unwrap()).collect();
    assert_eq!(patterns, [".slang/server.json", ".slang/local/server.json", "**/*.f", "**/*.sv", "**/*.svh", "**/*.v", "**/*.vh"]);
    assert_eq!(watchers[0]["globPattern"]["baseUri"], format!("file://{}", dir.display()));

    // A change it reports (a checkout, another editor) rebuilds, as a save does; other files don't.
    let changed = |name: &str| json!({ "uri": format!("file://{}/{name}", dir.display()), "type": 2 });
    client.notify("workspace/didChangeWatchedFiles", json!({ "changes": [changed("notes.txt")] }));
    client.notify("workspace/didChangeWatchedFiles", json!({ "changes": [changed("notes.txt"), changed("cone_test.sv"), changed("cone_test.f")] }));
    let started = Instant::now();
    while runs(&dir) < 2 {
        assert!(started.elapsed() < Duration::from_secs(10), "no rebuild after a watched file changed");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(600));
    assert_eq!(runs(&dir), 2);
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_rebuild_that_fails_after_a_good_one_tells_the_user_once_and_keeps_the_design() {
    let dir = workspace("rebuild-fails");
    let (mut client, handle) = start(&dir, fake_slang(&dir));
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);
    let config = dir.join(".slang/server.json");
    let good = fs::read_to_string(&config).unwrap();
    let messages = |client: &Client| -> Vec<String> {
        let shown = client.told.iter().filter(|told| told.method == "window/showMessage");
        shown.map(|told| format!("{}|{}", told.params["type"], told.params["message"].as_str().unwrap())).collect()
    };

    // The configuration is broken: the rebuild fails, the user is told, the last design answers.
    fs::write(&config, "{ not json").unwrap();
    assert_eq!(client.command("svtrace.reload", json!([]))["state"], "failed");
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);
    let told = messages(&client);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].starts_with("2|svtrace: the design could not be rebuilt; answers come from the last one that could. reading "), "{told:?}");
    // Still broken: not told again.
    assert_eq!(client.command("svtrace.reload", json!([]))["state"], "failed");
    client.command("svtrace.status", json!([]));
    assert_eq!(messages(&client).len(), 1);
    // Mended, then broken again: told again.
    fs::write(&config, good).unwrap();
    assert_eq!(client.command("svtrace.reload", json!([]))["state"], "ready");
    fs::write(&config, "{ not json").unwrap();
    assert_eq!(client.command("svtrace.reload", json!([]))["state"], "failed");
    client.command("svtrace.status", json!([]));
    assert_eq!(messages(&client).len(), 2);
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_first_build_that_fails_is_the_questions_error_not_a_message() {
    let dir = workspace("first-fails");
    fs::write(dir.join(".slang/server.json"), "{ not json").unwrap();
    let (mut client, handle) = start(&dir, fake_slang(&dir));
    assert!(client.command(DRIVERS, json!(["test.x1"]))["error"]["message"].as_str().unwrap().starts_with("svtrace: no design: reading "));
    client.command("svtrace.status", json!([]));
    assert!(client.told.is_empty(), "{:?}", client.told);
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn flags_that_svtrace_passes_too_reach_slang_once() {
    let dir = workspace("own-flags");
    fs::write(dir.join(".slang/server.json"), r#"{ "build": "cone_test.f", "flags": "-q -DX=1 --ast-json out.json --ast-json-source-info --disable-instance-caching" }"#).unwrap();
    let script = dir.join("args-slang");
    let dump = data_dir().join("cone_test.json");
    fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\nexit 1\n", dir.join("args").display(), dump.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let (mut client, handle) = start(&dir, script);
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);
    let args = fs::read_to_string(dir.join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let build = dir.join("cone_test.f");
    let expected = [
        "-DX=1", "-f", build.to_str().unwrap(), "--ast-json", "-",
        "--ast-json-source-info", "--ast-json-detailed-types", "--ast-json-compact", "--disable-instance-caching", "-q",
    ];
    assert_eq!(args, expected);
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn output_that_is_not_a_dump_and_a_slang_that_fails_are_told_apart() {
    let dir = workspace("not-a-dump");
    let script = |name: &str, body: &str| {
        let script = dir.join(name);
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    };
    // (The dump is read as slang writes it; a megabyte of something else must not leave slang stuck writing.)
    let error = svtrace::build(&script("garbage", "yes 'not json' | head -c 1000000\nexit 0"), &dir).err().unwrap();
    assert!(format!("{error:#}").starts_with("reading slang's AST dump: "), "{error:#}");
    let error = svtrace::build(&script("cut-short", "printf '{\"design\": {'\necho 'error: boom' >&2\nexit 3"), &dir).err().unwrap();
    assert_eq!(format!("{error:#}"), "slang failed (3): error: boom");
    let error = svtrace::build(&script("nothing", "exit 0"), &dir).err().unwrap();
    assert_eq!(format!("{error:#}"), "slang failed (0): ");
    let _ = fs::remove_dir_all(dir);
}

/// The real slang takes a `flags` line that repeats svtrace's own (it refuses an option given twice).
#[test]
fn real_slang_builds_with_flags_that_repeat_svtraces_own() {
    let slang = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/slang/slang");
    if !slang.exists() {
        eprintln!("skipped: no slang at {}", slang.display());
        return;
    }
    let dir = workspace("real-flags");
    fs::write(dir.join(".slang/server.json"), r#"{ "build": "cone_test.f", "flags": "-q --ast-json out.json --ast-json-compact" }"#).unwrap();
    let built = svtrace::build(&slang, &dir).expect("slang runs with the repeated flags left out");
    assert!(built.graph.signals().count() > 30);
    assert!(!dir.join("out.json").exists());
    let _ = fs::remove_dir_all(dir);
}

/// The real slang on cone_test.sv gives the answers of the checked-in dump.
#[test]
fn real_slang_gives_the_same_answers_as_the_checked_in_dump() {
    let slang = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/slang/slang");
    if !slang.exists() {
        eprintln!("skipped: no slang at {}", slang.display());
        return;
    }
    let dir = workspace("real");
    let built = svtrace::build(&slang, &dir).expect("slang runs on cone_test.sv");
    let fixture = common::fixture("cone_test");
    let answers = |graph: &svtrace::graph::Graph, root: &Path| -> Vec<String> {
        let root = root.display().to_string();
        let mut answers = Vec::new();
        for path in graph.signals() {
            for command in [DRIVERS, LOADS, "slang.getDriversWithConstants"] {
                let answer = svtrace::trace::execute(graph, command, path);
                answers.push(format!("{command} {path} {}", format!("{answer:?}").replace(&root, "<root>")));
            }
        }
        answers
    };
    assert_eq!(answers(&built.graph, &dir), answers(&fixture.graph, &data_dir()));
    assert!(built.graph.signals().count() > 30);
    let _ = fs::remove_dir_all(dir);
}

/// [`fake_slang`], which also records its arguments, one run a line, in `args`.
fn recording_slang(dir: &Path) -> PathBuf {
    let script = dir.join("fake-slang");
    let runs = dir.join("runs");
    let dump = data_dir().join("cone_test.json");
    fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> '{}'\necho run >> '{}'\ncat '{}'\nexit 1\n", dir.join("args").display(), runs.display(), dump.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The `-f`/`-F` file of each slang run.
fn builds(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("args"))
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let words: Vec<&str> = line.split(' ').collect();
            let at = words.iter().position(|word| *word == "-f" || *word == "-F").expect("a build file");
            words[at + 1].to_string()
        })
        .collect()
}

#[test]
fn use_build_switches_the_build_file_to_none_and_back() {
    let dir = workspace("use-build");
    fs::write(dir.join("other.f"), "cone_test.sv\n").unwrap();
    let (mut client, handle) = start(&dir, recording_slang(&dir));
    client.command(DRIVERS, json!(["test.x1"]));
    let status = client.command("svtrace.status", json!([]));
    assert_eq!((status["state"].as_str(), status["build"].clone()), (Some("ready"), json!({ "choice": "config" })));
    assert_eq!(status["design"], json!(true));
    assert_eq!(builds(&dir), [dir.join("cone_test.f").display().to_string()]);

    // A build file, relative to the workspace: elaborated in place of server.json's.
    let status = client.command("svtrace.useBuild", json!([{ "file": "other.f" }]));
    assert_eq!(status["state"], "ready");
    assert_eq!(status["build"], json!({ "choice": "file", "file": "other.f" }));
    assert_eq!(builds(&dir)[1], dir.join("other.f").display().to_string());
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);

    // A rebuild on a change uses the chosen one.
    client.command("svtrace.reload", json!([]));
    assert_eq!(builds(&dir)[2], dir.join("other.f").display().to_string());

    // None: no slang run, no design, and every question says so.
    let runs_before = runs(&dir);
    let status = client.command("svtrace.useBuild", json!([{ "file": null }]));
    assert_eq!((status["state"].as_str(), status["build"].clone()), (Some("failed"), json!({ "choice": "none" })));
    assert_eq!(status["error"], "no build chosen");
    assert_eq!(status["design"], json!(false));
    assert_eq!(runs(&dir), runs_before);
    for command in [DRIVERS, "slang.getScopes"] {
        let answer = client.command(command, json!([""]));
        assert_eq!(answer["error"]["message"], "svtrace: no design: no build chosen", "{command}: {answer}");
    }
    // (Not told as a failed rebuild: there was nothing to rebuild.)
    assert!(!client.told.iter().any(|n| n.method == "window/showMessage"), "{:?}", client.told);

    // Back to the configured build file is a chosen one too: the same file by name.
    let status = client.command("svtrace.useBuild", json!([{ "file": "cone_test.f" }]));
    assert_eq!(status["state"], "ready");
    assert_eq!(client.command(DRIVERS, json!(["test.x1"])).as_array().unwrap().len(), 1);

    // A file that is not there: the build fails and says so, and the old design is not kept (it is of another build).
    let status = client.command("svtrace.useBuild", json!([{ "file": "missing.f" }]));
    assert_eq!(status["state"], "failed");
    assert_eq!(status["error"], "build file not found: missing.f");
    let answer = client.command(DRIVERS, json!(["test.x1"]));
    assert!(answer["error"]["message"].as_str().unwrap().starts_with("svtrace: no design: build file not found"), "{answer}");
    assert!(!client.told.iter().any(|n| n.method == "window/showMessage"));

    // A wrong argument is an error and changes nothing.
    let answer = client.command("svtrace.useBuild", json!([{ "file": 3 }]));
    assert!(answer["error"]["message"].as_str().unwrap().contains("svtrace.useBuild"), "{answer}");
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_chosen_build_survives_a_change_of_server_json_unless_it_is_the_old_configured_one() {
    let dir = workspace("use-build-config");
    for name in ["other.f", "third.f", "fourth.f"] {
        fs::write(dir.join(name), "cone_test.sv\n").unwrap();
    }
    let (mut client, handle) = start(&dir, recording_slang(&dir));
    client.command(DRIVERS, json!(["test.x1"]));
    let wait_runs = |dir: &Path, n: usize| {
        let started = Instant::now();
        while runs(dir) < n {
            assert!(started.elapsed() < Duration::from_secs(10), "no rebuild");
            thread::sleep(Duration::from_millis(20));
        }
        // (Nothing more is coming.)
        thread::sleep(Duration::from_millis(500));
    };
    let save_config = |client: &Client, build: &str| {
        fs::write(dir.join(".slang/server.json"), format!(r#"{{ "build": "{build}" }}"#)).unwrap();
        client.notify("textDocument/didSave", json!({ "textDocument": { "uri": format!("file://{}/.slang/server.json", dir.display()) } }));
    };

    // A build the client chose stays when the configured one changes.
    client.command("svtrace.useBuild", json!([{ "file": "other.f" }]));
    let before = runs(&dir);
    save_config(&client, "third.f");
    wait_runs(&dir, before + 1);
    assert_eq!(builds(&dir).last().unwrap(), &dir.join("other.f").display().to_string());
    assert_eq!(client.command("svtrace.status", json!([]))["build"], json!({ "choice": "file", "file": "other.f" }));

    // The configured one (now third.f), chosen by name, follows the next change.
    client.command("svtrace.useBuild", json!([{ "file": "third.f" }]));
    let before = runs(&dir);
    save_config(&client, "fourth.f");
    wait_runs(&dir, before + 1);
    assert_eq!(builds(&dir).last().unwrap(), &dir.join("fourth.f").display().to_string());
    assert_eq!(client.command("svtrace.status", json!([]))["build"], json!({ "choice": "config" }));

    // No design for a top file stays none.
    client.command("svtrace.useBuild", json!([{ "file": null }]));
    save_config(&client, "third.f");
    thread::sleep(Duration::from_millis(600));
    assert_eq!(client.command("svtrace.status", json!([]))["build"], json!({ "choice": "none" }));

    // Explore mode goes back to the configured build, as slang-server's reload does.
    client.command("svtrace.useBuild", json!([{ "file": null, "explore": true }]));
    assert_eq!(client.command("svtrace.status", json!([]))["build"], json!({ "choice": "none" }));
    let before = runs(&dir);
    save_config(&client, "fourth.f");
    wait_runs(&dir, before + 1);
    assert_eq!(builds(&dir).last().unwrap(), &dir.join("fourth.f").display().to_string());
    assert_eq!(client.command("svtrace.status", json!([]))["build"], json!({ "choice": "config" }));
    assert!(client.command("svtrace.useBuild", json!([{ "file": null, "explore": 1 }]))["error"]["message"].as_str().unwrap().contains("explore = bool"));
    finish(client, handle);
    let _ = fs::remove_dir_all(dir);
}
