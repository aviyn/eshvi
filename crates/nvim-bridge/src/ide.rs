//! Claude Code's IDE connection, served by eshvi the way the VS Code
//! extension serves it: a WebSocket (subprotocol `mcp`) on 127.0.0.1 that
//! Claude Code finds through a lock file, `~/.claude/ide/<port>.lock`
//! (`$CLAUDE_CONFIG_DIR/ide` if set), and connects to with the lock's
//! token in `X-Claude-Code-Ide-Authorization`. Agents started from the
//! agent panel get `CLAUDE_CODE_SSE_PORT` and connect at once; others pick
//! "Eshvi" with `/ide`.
//!
//! Over it, JSON-RPC (MCP): eshvi tells Claude the current file and
//! selection (`selection_changed`) and sends files or lines as context
//! (`at_mentioned`); Claude calls eshvi's tools (diagnostics, open editors,
//! the diff view for proposed edits). Names and shapes as Claude Code 2.1
//! uses them (checked against its binary): `ide_connected {pid}` after
//! connecting, `openDiff` answered with `FILE_SAVED` + the final text,
//! `DIFF_REJECTED` or `TAB_CLOSED`, then `close_tab`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use nvim_rs::Value;
use serde_json::{json, Value as Json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::{http, Message};

use crate::Nvim;

const TOKEN_HEADER: &str = "x-claude-code-ide-authorization";

/// The running server; cheap to clone.
#[derive(Clone)]
pub struct Ide {
    inner: Arc<Inner>,
}

struct Inner {
    port: u16,
    token: String,
    lock: PathBuf,
    nvim: Nvim,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    clients: HashMap<u64, Client>,
    next_client: u64,
    workspace: Vec<String>,
    /// The last `selection_changed` sent, for getLatestSelection.
    selection: Option<Json>,
    /// Diff views waiting for the user, by the `openDiff` call that opened
    /// each (not by tab name: a second call with the same name closes the
    /// first view, and that answer is the first call's).
    diffs: HashMap<u64, oneshot::Sender<Json>>,
    next_diff: u64,
}

struct Client {
    tx: mpsc::UnboundedSender<Message>,
    /// Claude Code's process (from `ide_connected`).
    pid: Option<i64>,
}

impl Ide {
    /// Listen on a free local port and write the lock file.
    pub async fn start(nvim: Nvim, workspace: Vec<String>, dir: Option<PathBuf>) -> anyhow::Result<Ide> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("random token: {e}"))?;
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let dir = dir.unwrap_or_else(lock_dir);
        remove_stale_locks(&dir);
        let lock = dir.join(format!("{port}.lock"));
        let ide = Ide {
            inner: Arc::new(Inner {
                port,
                token,
                lock,
                nvim,
                state: Mutex::new(State { workspace, ..Default::default() }),
            }),
        };
        ide.write_lock()?;
        let accept = ide.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let ide = accept.clone();
                tokio::spawn(async move {
                    if let Err(e) = ide.serve(stream).await {
                        log::debug!("ide connection: {e}");
                    }
                });
            }
        });
        log::info!("ide server on 127.0.0.1:{port}");
        Ok(ide)
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// The folders Claude Code matches its working directory against.
    pub fn set_workspace(&self, folders: Vec<String>) {
        self.inner.state.lock().unwrap().workspace = folders;
        if let Err(e) = self.write_lock() {
            log::warn!("ide lock file: {e}");
        }
    }

    /// Remove the lock file (Neovim exited).
    pub fn shutdown(&self) {
        let _ = std::fs::remove_file(&self.inner.lock);
    }

    fn write_lock(&self) -> std::io::Result<()> {
        let workspace = self.inner.state.lock().unwrap().workspace.clone();
        let lock = json!({
            "pid": std::process::id(),
            "workspaceFolders": workspace,
            "ideName": "Eshvi",
            "transport": "ws",
            "runningInWindows": cfg!(windows),
            "authToken": self.inner.token,
        });
        if let Some(dir) = self.inner.lock.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Written aside and renamed, so Claude Code never reads half of it;
        // only the user may read the token, from the moment the file is
        // there (made anew: an old one would keep its mode).
        let tmp = self.inner.lock.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&tmp)?, lock.to_string().as_bytes())?;
        std::fs::rename(&tmp, &self.inner.lock)
    }

    /// The user's selection or current file changed (from Lua).
    pub fn selection_changed(&self, params: Json) {
        self.inner.state.lock().unwrap().selection = Some(params.clone());
        self.broadcast("selection_changed", params, None);
    }

    /// Send a file or lines to Claude as context: to the agent with process
    /// `pid` when it is connected, else to every connected one.
    pub fn at_mentioned(&self, params: Json, pid: Option<i64>) {
        self.broadcast("at_mentioned", params, pid);
    }

    /// The user decided on a diff view (from Lua): `result` is the tool
    /// result's content, for the `openDiff` call `id` that opened the view.
    pub fn diff_done(&self, id: u64, result: Json) {
        if let Some(tx) = self.inner.state.lock().unwrap().diffs.remove(&id) {
            let _ = tx.send(result);
        }
    }

    fn broadcast(&self, method: &str, params: Json, pid: Option<i64>) {
        let text = json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string();
        let state = self.inner.state.lock().unwrap();
        let target = pid.filter(|p| state.clients.values().any(|c| c.pid == Some(*p)));
        for c in state.clients.values() {
            if target.is_none() || c.pid == target {
                let _ = c.tx.send(Message::text(text.clone()));
            }
        }
    }

    // (tungstenite's handshake callback returns its large ErrorResponse.)
    #[allow(clippy::result_large_err)]
    async fn serve(&self, stream: TcpStream) -> anyhow::Result<()> {
        let token = self.inner.token.clone();
        let check = move |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
            let sent = req.headers().get(TOKEN_HEADER).and_then(|v| v.to_str().ok());
            if sent != Some(token.as_str()) {
                let mut err = ErrorResponse::new(Some("bad token".into()));
                *err.status_mut() = http::StatusCode::UNAUTHORIZED;
                return Err(err);
            }
            let wants_mcp = req
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(',').any(|p| p.trim() == "mcp"));
            if wants_mcp {
                resp.headers_mut().insert("sec-websocket-protocol", http::HeaderValue::from_static("mcp"));
            }
            Ok(resp)
        };
        let ws = tokio_tungstenite::accept_hdr_async(stream, check).await?;
        let (mut sink, mut source) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        let id = {
            let mut state = self.inner.state.lock().unwrap();
            state.next_client += 1;
            let id = state.next_client;
            state.clients.insert(id, Client { tx: tx.clone(), pid: None });
            id
        };
        log::info!("ide client {id} connected");
        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });
        while let Some(msg) = source.next().await {
            // A read error (a killed agent) ends it as a close does: the
            // client must still come off the list below.
            let msg = match msg {
                Ok(msg) => msg,
                Err(e) => {
                    log::debug!("ide client {id}: {e}");
                    break;
                }
            };
            match msg {
                Message::Text(text) => {
                    let Ok(msg) = serde_json::from_str::<Json>(&text) else { continue };
                    self.handle(id, msg, &tx);
                }
                Message::Ping(p) => {
                    let _ = tx.send(Message::Pong(p));
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        self.inner.state.lock().unwrap().clients.remove(&id);
        writer.abort();
        log::info!("ide client {id} disconnected");
        Ok(())
    }

    fn handle(&self, client: u64, msg: Json, tx: &mpsc::UnboundedSender<Message>) {
        let method = msg["method"].as_str().unwrap_or_default().to_owned();
        let Some(id) = msg.get("id").cloned() else {
            // Notifications.
            if method == "ide_connected" {
                if let Some(c) = self.inner.state.lock().unwrap().clients.get_mut(&client) {
                    c.pid = msg["params"]["pid"].as_i64();
                }
            }
            return;
        };
        let reply = |result: Result<Json, (i64, String)>| {
            let body = match result {
                Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
            };
            let _ = tx.send(Message::text(body.to_string()));
        };
        match method.as_str() {
            "initialize" => {
                let version = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
                reply(Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "eshvi", "version": env!("CARGO_PKG_VERSION") },
                })))
            }
            "ping" => reply(Ok(json!({}))),
            "tools/list" => reply(Ok(json!({ "tools": tools() }))),
            "tools/call" => {
                let ide = self.clone();
                let tx = tx.clone();
                let name = msg["params"]["name"].as_str().unwrap_or_default().to_owned();
                let args = msg["params"]["arguments"].clone();
                tokio::spawn(async move {
                    let result = match ide.call(&name, args).await {
                        Ok(content) => json!({ "content": content }),
                        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                    };
                    let body = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    let _ = tx.send(Message::text(body.to_string()));
                });
            }
            _ => reply(Err((-32601, format!("method not found: {method}")))),
        }
    }

    /// Run a tool; the result is MCP content (a list of text items).
    async fn call(&self, name: &str, args: Json) -> Result<Json, String> {
        let text = |s: String| json!([{ "type": "text", "text": s }]);
        match name {
            "openDiff" => {
                let (tx, rx) = oneshot::channel();
                let id = {
                    let mut state = self.inner.state.lock().unwrap();
                    state.next_diff += 1;
                    let id = state.next_diff;
                    state.diffs.insert(id, tx);
                    id
                };
                // The view answers with this id (`eshvi_ide_diff`).
                let mut args = args;
                if let Some(map) = args.as_object_mut() {
                    map.insert("call_id".into(), json!(id));
                }
                if let Err(e) = self.lua("open_diff", vec![args]).await {
                    self.inner.state.lock().unwrap().diffs.remove(&id);
                    return Err(e);
                }
                // Answered when the user accepts or rejects (`diff_done`).
                rx.await.map_err(|_| "the diff view went away".to_owned())
            }
            "close_tab" => {
                let tab = args["tab_name"].as_str().unwrap_or_default().to_owned();
                self.lua("close_diff", vec![json!(tab)]).await?;
                Ok(text("TAB_CLOSED".into()))
            }
            "closeAllDiffTabs" => {
                let n = self.lua("close_all_diffs", vec![]).await?;
                Ok(text(format!("CLOSED_{}_DIFF_TABS", n.as_i64().unwrap_or(0))))
            }
            "getLatestSelection" | "getCurrentSelection" => {
                let selection = self.inner.state.lock().unwrap().selection.clone();
                Ok(text(selection.unwrap_or_else(|| json!({ "success": false })).to_string()))
            }
            "getWorkspaceFolders" => {
                let folders = self.inner.state.lock().unwrap().workspace.clone();
                let list: Vec<Json> =
                    folders.iter().map(|f| json!({ "name": basename(f), "uri": format!("file://{f}"), "path": f })).collect();
                Ok(text(json!({ "success": true, "folders": list, "rootPath": folders.first() }).to_string()))
            }
            "getDiagnostics" | "getOpenEditors" | "checkDocumentDirty" | "saveDocument" | "openFile" => {
                let fun = match name {
                    "getDiagnostics" => "diagnostics",
                    "getOpenEditors" => "open_editors",
                    "checkDocumentDirty" => "document_dirty",
                    "saveDocument" => "save_document",
                    _ => "open_file",
                };
                Ok(text(self.lua(fun, vec![args]).await?.to_string()))
            }
            _ => Err(format!("unknown tool {name}")),
        }
    }

    /// Call `Eshvi.ide.<fun>(args...)` in Neovim.
    async fn lua(&self, fun: &str, args: Vec<Json>) -> Result<Json, String> {
        let code = format!("return Eshvi.ide.{fun}(...)");
        let args = args.iter().map(to_value).collect();
        match self.inner.nvim.exec_lua(&code, args).await {
            Ok(v) => Ok(to_json(&v)),
            Err(e) => Err(format!("{fun}: {e}")),
        }
    }
}

fn lock_dir() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("ide"),
        None => {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default();
            PathBuf::from(home).join(".claude").join("ide")
        }
    }
}

/// Remove the lock files of Eshvis that are gone (killed: one that quits
/// removes its own), so Claude Code's `/ide` doesn't list them. Only ours,
/// and only when the process that wrote one no longer exists.
fn remove_stale_locks(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "lock")) {
        let Some(info) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Json>(&t).ok()) else { continue };
        let Some(pid) = info["pid"].as_i64() else { continue };
        if info["ideName"] == "Eshvi" && !process_exists(pid) {
            log::info!("ide: removing the lock of a process that is gone: {}", path.display());
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether process `pid` exists (when it can't be told: yes).
fn process_exists(pid: i64) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return true };
        if pid <= 0 {
            return true;
        }
        // Signal 0 sends nothing; "no such process" is the only answer that says it's gone.
        // SAFETY: `kill` with signal 0 only checks; it takes no pointers.
        let gone = unsafe { libc::kill(pid, 0) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        !gone
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The tools Claude Code may call.
fn tools() -> Json {
    let tool = |name: &str, description: &str, props: Json, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "properties": props, "required": required },
        })
    };
    let string = json!({ "type": "string" });
    json!([
        tool(
            "openDiff",
            "Show a proposed edit as a diff; answers when the user accepts or rejects it.",
            json!({
                "old_file_path": string, "new_file_path": string,
                "new_file_contents": string, "tab_name": string,
            }),
            &["old_file_path", "new_file_path", "new_file_contents", "tab_name"],
        ),
        tool("close_tab", "Close a diff view.", json!({ "tab_name": string }), &["tab_name"]),
        tool("closeAllDiffTabs", "Close all diff views.", json!({}), &[]),
        tool(
            "getDiagnostics",
            "Diagnostics (errors, warnings) from the editor's language servers, for one file (a file:// URI) or all.",
            json!({ "uri": string }),
            &[],
        ),
        tool("getCurrentSelection", "The current selection in the editor.", json!({}), &[]),
        tool("getLatestSelection", "The most recent selection in the editor.", json!({}), &[]),
        tool("getOpenEditors", "The files open in the editor.", json!({}), &[]),
        tool("getWorkspaceFolders", "The workspace folders.", json!({}), &[]),
        tool("checkDocumentDirty", "Whether a file has unsaved changes.", json!({ "filePath": string }), &["filePath"]),
        tool("saveDocument", "Save a file open in the editor.", json!({ "filePath": string }), &["filePath"]),
        tool(
            "openFile",
            "Open a file in the editor.",
            json!({ "filePath": string, "startText": string, "endText": string }),
            &["filePath"],
        ),
    ])
}

pub(crate) fn to_value(j: &Json) -> Value {
    match j {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::from(*b),
        Json::Number(n) => n.as_i64().map(Value::from).or_else(|| n.as_f64().map(Value::from)).unwrap_or(Value::Nil),
        Json::String(s) => Value::from(s.as_str()),
        Json::Array(a) => Value::Array(a.iter().map(to_value).collect()),
        Json::Object(o) => Value::Map(o.iter().map(|(k, v)| (Value::from(k.as_str()), to_value(v))).collect()),
    }
}

pub(crate) fn to_json(v: &Value) -> Json {
    match v {
        Value::Nil => Json::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => i.as_i64().map_or_else(|| json!(i.as_u64()), |i| json!(i)),
        Value::F32(f) => json!(f),
        Value::F64(f) => json!(f),
        // (Not UTF-8, a file name for one: what can be read of it, not nothing.)
        Value::String(s) => json!(String::from_utf8_lossy(s.as_bytes())),
        Value::Binary(b) => json!(String::from_utf8_lossy(b)),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => {
            // An empty Lua table comes as an empty map; keep it a list.
            if m.is_empty() {
                return json!([]);
            }
            Json::Object(m.iter().map(|(k, v)| (k.as_str().map_or_else(|| k.to_string(), str::to_owned), to_json(v))).collect())
        }
        Value::Ext(..) => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let j = json!({ "a": [1, "x", true, null], "b": { "c": 2.5 } });
        assert_eq!(to_json(&to_value(&j)), j);
        assert_eq!(basename("/a/b/"), "b");
    }

    #[cfg(unix)]
    #[test]
    fn locks_of_processes_that_are_gone_are_removed() {
        let dir = std::env::temp_dir().join(format!("eshvi-ide-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A process that has ended (and been waited for): its pid is free.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let gone = child.id();
        child.wait().unwrap();
        let lock = |name: &str, pid: u32, ide: &str| {
            std::fs::write(dir.join(name), json!({ "pid": pid, "ideName": ide }).to_string()).unwrap();
        };
        lock("1.lock", gone, "Eshvi");
        lock("2.lock", std::process::id(), "Eshvi");
        lock("3.lock", gone, "Visual Studio Code");
        std::fs::write(dir.join("4.lock"), "not json").unwrap();
        remove_stale_locks(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["2.lock", "3.lock", "4.lock"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
