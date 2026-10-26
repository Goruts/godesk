//! Lets the programs in Godesk's terminals reach each other, whatever they
//! are (Claude Code, Codex, a script): every shell gets a `godesk` command
//! on its PATH that talks to this local endpoint.
//!
//! - `godesk list` names the terminals of the caller's workspace and what
//!   runs in them.
//! - `godesk send <terminal> <message>` types the message into that terminal
//!   and presses Enter, as a person would.
//! - `godesk read <terminal> [lines]` prints the end of its output.
//!
//! Each use is an interaction, so it creates or refreshes the detected link
//! between the two terminals (sender → receiver; read terminal → reader).
//!
//! The endpoint is plain HTTP on 127.0.0.1 with a random token that only
//! Godesk's shells know (`GODESK_TOKEN`). A web page cannot call it: it
//! cannot send the token header without a CORS preflight, which is never
//! answered.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::agent_session::{AgentSessions, AgentSink};
use crate::core::{Shared, now_ms};
use crate::pty::PtyManager;
use crate::text::AnsiStripper;

/// Largest request body accepted (a message).
const MAX_BODY_BYTES: usize = 256 * 1024;
/// Lines `godesk read` prints by default.
const DEFAULT_READ_LINES: usize = 40;
/// Pause between a sent message and its Enter: TUIs such as Claude Code take
/// text and Enter arriving together for a paste, and would not submit it.
const ENTER_DELAY: Duration = Duration::from_millis(150);
/// How long a client may take to send its request. A client that opens a
/// connection and says nothing must not pin a thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections served at the same time.
const MAX_CONNECTIONS: usize = 32;
/// Pause after a failed accept, so a persistent failure is not a busy loop.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);
/// Bytes of an oversized body read anyway, so the client can read the
/// answer instead of a broken connection.
const DRAIN_BYTES: u64 = 64 * 1024;

/// What shells need to reach the endpoint: environment variables and the
/// folder with the `godesk` command, for the front of their PATH.
pub struct BridgeEnv {
    pub vars: Vec<(String, String)>,
    pub bin_dir: PathBuf,
}

/// Binds the endpoint and writes the `godesk` command into `data_dir/bin`.
/// Requests are served once [`serve`] is given the terminals.
pub fn bind(data_dir: &Path) -> std::io::Result<(TcpListener, BridgeEnv)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let bin_dir = data_dir.join("bin");
    write_commands(&bin_dir, port, &token)?;
    let vars = vec![("GODESK_PORT".to_owned(), port.to_string()), ("GODESK_TOKEN".to_owned(), token)];
    Ok((listener, BridgeEnv { vars, bin_dir }))
}

/// What the bridge reaches: the terminals' shells, and the agents some of
/// them show instead (a message to such a terminal goes to its agent).
#[derive(Clone)]
pub struct Reach {
    pub ptys: PtyManager,
    pub agents: AgentSessions,
    pub agent_sink: Arc<dyn AgentSink>,
    /// The frecency store, so the `z` jump can resolve a folder (see [`crate::dirs`]).
    pub dirs: Arc<crate::dirs::DirStore>,
    /// Knowledge bases' indexes and extracted text (see [`crate::knowledge`]).
    pub kb: crate::knowledge::KbStore,
}

/// Answers requests on `listener`, one thread per connection, up to
/// [`MAX_CONNECTIONS`] at the same time.
pub fn serve(listener: TcpListener, token: String, shared: Arc<Shared>, reach: Reach) {
    serve_with_limit(listener, token, shared, reach, MAX_CONNECTIONS)
}

/// Answers requests on `listener`, refusing the ones that come when
/// `max` connections are already being served.
fn serve_with_limit(
    listener: TcpListener,
    token: String,
    shared: Arc<Shared>,
    reach: Reach,
    max: usize,
) {
    let connections = Arc::new(Connections::new(max));
    let spawned = thread::Builder::new().name("godesk-bridge".into()).spawn(move || {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    // A failed accept must not become a busy loop.
                    eprintln!("godesk: cannot accept a connection: {err}");
                    thread::sleep(ACCEPT_RETRY);
                    continue;
                }
            };
            let Some(slot) = connections.enter() else {
                let _ = respond(&stream, "503 Service Unavailable", "godesk: too many connections at once");
                continue;
            };
            let (token, shared, reach) = (token.clone(), shared.clone(), reach.clone());
            let _ = thread::Builder::new().name("godesk-bridge-request".into()).spawn(move || {
                let _slot = slot;
                if let Err(err) = handle(stream, &token, &shared, &reach) {
                    eprintln!("godesk: bridge request failed: {err}");
                }
            });
        }
    });
    if let Err(err) = spawned {
        eprintln!("godesk: cannot start the terminal bridge: {err}");
    }
}

/// The connections being served at the same time.
struct Connections {
    open: AtomicUsize,
    max: usize,
}

impl Connections {
    fn new(max: usize) -> Self {
        Self { open: AtomicUsize::new(0), max }
    }

    /// Takes a slot for a connection, if there is one free.
    fn enter(self: &Arc<Self>) -> Option<Slot> {
        let mut current = self.open.load(Ordering::Relaxed);
        loop {
            if current >= self.max {
                return None;
            }
            match self.open.compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(Slot(self.clone())),
                Err(actual) => current = actual,
            }
        }
    }
}

/// The slot a connection holds while it is served; it is given back when
/// the connection ends.
struct Slot(Arc<Connections>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A request that cannot be served: the status to answer and the text.
#[derive(Debug)]
struct RequestError {
    status: &'static str,
    text: String,
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

fn read_request(stream: &TcpStream) -> Result<Request, RequestError> {
    read_request_in(stream, READ_TIMEOUT)
}

/// Reads a request, giving up after `timeout`: a client that opens a
/// connection and says nothing is answered, not waited for.
fn read_request_in(stream: &TcpStream, timeout: Duration) -> Result<Request, RequestError> {
    if let Err(err) = stream.set_read_timeout(Some(timeout)) {
        eprintln!("godesk: cannot set the read timeout: {err}");
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => {
            return Err(RequestError { status: "400 Bad Request", text: "godesk: empty request".into() });
        }
        Ok(_) => {}
        Err(err) => return Err(read_error(err)),
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().split('?').next().unwrap_or_default().to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => return Err(read_error(err)),
        }
        if header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: u64 = match headers.get("content-length") {
        Some(value) => match value.parse::<u64>() {
            Ok(length) => length,
            Err(_) => {
                return Err(RequestError { status: "400 Bad Request", text: "godesk: bad Content-Length".into() });
            }
        },
        None => 0,
    };
    if length > MAX_BODY_BYTES as u64 {
        // The message is too long: read what comes so the client can read
        // the answer, and do not type a truncated message anywhere.
        drain(&mut reader, length);
        return Err(RequestError {
            status: "413 Payload Too Large",
            text: format!("godesk: the message is longer than {MAX_BODY_BYTES} bytes"),
        });
    }
    let mut body = vec![0; length as usize];
    if let Err(err) = reader.read_exact(&mut body) {
        return Err(read_error(err));
    }
    Ok(Request { method, path, headers, body: String::from_utf8_lossy(&body).into_owned() })
}

/// A read that failed: a timeout is the client's fault, anything else is a
/// broken request.
fn read_error(err: std::io::Error) -> RequestError {
    if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
        RequestError { status: "408 Request Timeout", text: "godesk: the request took too long".into() }
    } else {
        RequestError { status: "400 Bad Request", text: format!("godesk: cannot read the request: {err}") }
    }
}

/// Reads and throws away the body of a request that is not going to be
/// served, so the client can read the answer.
fn drain(reader: &mut impl Read, length: u64) {
    let mut buffer = [0u8; 4096];
    let mut left = length.min(DRAIN_BYTES);
    while left > 0 {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => left -= read as u64,
        }
    }
}

/// Compares two secrets without telling where they differ.
fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in given.iter().zip(expected) {
        difference |= a ^ b;
    }
    difference == 0
}

fn respond(mut stream: &TcpStream, status: &str, text: &str) -> std::io::Result<()> {
    let text = if text.ends_with('\n') { text.to_owned() } else { format!("{text}\n") };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )?;
    stream.flush()
}

fn handle(stream: TcpStream, token: &str, shared: &Shared, reach: &Reach) -> std::io::Result<()> {
    let request = match read_request(&stream) {
        Ok(request) => request,
        Err(err) => return respond(&stream, err.status, &err.text),
    };
    let given = request.headers.get("x-godesk-token").map(String::as_bytes).unwrap_or_default();
    if !same_secret(given, token.as_bytes()) {
        return respond(&stream, "403 Forbidden", "godesk: not called from a Godesk terminal");
    }
    let from = request.headers.get("x-godesk-from").cloned().unwrap_or_default();
    // A handler panic (e.g. a malformed document deep in a parser) must not tear
    // down this thread and reset the caller's connection, which surfaces as an
    // opaque network error. Catch it and answer with a 500 the agent can read.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/list") => Ok(list(shared, reach, &from)),
            ("POST", "/send") => send(shared, reach, &from, &request.body),
            ("POST", "/read") => {
                let lines =
                    request.headers.get("x-godesk-lines").and_then(|l| l.parse().ok()).unwrap_or(DEFAULT_READ_LINES);
                read(shared, reach, &from, request.body.trim(), lines)
            }
            ("POST", "/dir") => dir(reach, request.body.trim()),
            ("GET", "/kb/list") => Ok(kb_list(shared, reach, &from)),
            ("POST", "/kb/read") => kb_read(shared, reach, &from, &request.body),
            ("POST", "/kb/search") => kb_search(shared, reach, &from, request.body.trim()),
            _ => Err(format!("godesk: unknown request {} {}", request.method, request.path)),
        }
    }));
    match result {
        Ok(Ok(text)) => respond(&stream, "200 OK", &text),
        Ok(Err(text)) => respond(&stream, "400 Bad Request", &text),
        Err(_) => respond(&stream, "500 Internal Server Error", "godesk: the request hit an internal error"),
    }
}

/// A terminal as the caller sees it.
#[derive(Debug)]
struct Target {
    id: String,
    name: String,
    workspace_id: String,
}

/// The terminals the caller can name: its own workspace's, or every terminal
/// when the caller is not one of Godesk's. Naming one is not the same as
/// being allowed to reach it (see [`reachable`]).
fn terminals(shared: &Shared, from: &str) -> Vec<Target> {
    let model = shared.model();
    let own = model.workspace_of(from).map(|w| w.id.clone());
    model
        .workspaces()
        .iter()
        .filter(|w| own.as_ref().is_none_or(|id| *id == w.id))
        .flat_map(|w| {
            w.terminals.iter().map(|t| Target { id: t.id.clone(), name: t.name.clone(), workspace_id: w.id.clone() })
        })
        .collect()
}

/// Whether `from` is one of Godesk's terminals, and so bound by the arrows.
/// A caller that is not (an external tool using the `godesk` command) is not
/// gated and reaches every terminal.
fn gated(shared: &Shared, from: &str) -> bool {
    shared.model().workspace_of(from).is_some()
}

/// The terminals an arrow connects to the caller: the only ones it may send
/// to or read from. Drawing or deleting an arrow in Godesk is what opens or
/// closes the channel.
fn reachable(shared: &Shared, from: &str) -> Vec<Target> {
    let gated = gated(shared, from);
    terminals(shared, from).into_iter().filter(|t| t.id != from && (!gated || shared.model().are_linked(from, &t.id))).collect()
}

/// Rejects a target the caller is not allowed to reach, so send and read
/// obey the arrows even though any terminal can be named.
fn ensure_reachable(shared: &Shared, from: &str, target: &Target) -> Result<(), String> {
    if !gated(shared, from) || shared.model().are_linked(from, &target.id) {
        return Ok(());
    }
    Err(format!(
        "godesk: no arrow connects you to {}. Draw one between the terminals in Godesk to talk to it.",
        target.name
    ))
}

/// The terminal `query` names: its exact name (any case), its id, or the
/// only name containing it. A name that fits several terminals is refused:
/// the message could go to the wrong agent.
fn resolve(shared: &Shared, from: &str, query: &str) -> Result<Target, String> {
    let query = query.trim();
    let mut all = terminals(shared, from);
    let lower = query.to_lowercase();
    let exact: Vec<usize> =
        (0..all.len()).filter(|&i| all[i].name.to_lowercase() == lower || all[i].id == query).collect();
    let index = match exact.as_slice() {
        [only] => *only,
        [] => {
            let partial: Vec<usize> =
                (0..all.len()).filter(|&i| all[i].name.to_lowercase().contains(&lower) && !lower.is_empty()).collect();
            match partial.as_slice() {
                [only] => *only,
                _ => {
                    let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
                    return Err(format!(
                        "godesk: no single terminal is called \"{query}\". Terminals: {}",
                        names.join(", ")
                    ));
                }
            }
        }
        many => {
            let names: Vec<&str> = many.iter().map(|&i| all[i].name.as_str()).collect();
            return Err(format!(
                "godesk: \"{query}\" is the name of several terminals: {}. Name it by its id.",
                names.join(", ")
            ));
        }
    };
    let target = all.swap_remove(index);
    if target.id == from {
        return Err("godesk: that is this terminal".into());
    }
    Ok(target)
}

/// Records that `source` reached `target`, and tells the frontend.
fn touch(shared: &Shared, workspace_id: &str, source: &str, target: &str) {
    let mut model = shared.model();
    if model.touch_link(workspace_id, source, target, now_ms()) {
        let links = model.links_of(workspace_id);
        drop(model);
        shared.emit_links(workspace_id, links);
    }
}

/// Lights the arrow from a node (a knowledge base) to the caller.
///
/// The workspace is looked up in its own statement on purpose: writing
/// `if let Some(ws) = shared.model()...{ touch(..) }` would keep the model guard
/// alive for the whole `if let` body (Rust drops a scrutinee temporary only at
/// the end of the block), and `touch` locks the model again on the same thread —
/// a re-entrant lock that deadlocks forever and freezes every later request.
fn touch_node(shared: &Shared, node_id: &str, from: &str) {
    let workspace = shared.model().workspace_of_node(node_id).map(|w| w.id.clone());
    if let Some(workspace) = workspace {
        touch(shared, &workspace, node_id, from);
    }
}

fn list(shared: &Shared, reach: &Reach, from: &str) -> String {
    let reachable = reachable(shared, from);
    if reachable.is_empty() {
        return "No terminals are connected to you. Draw an arrow between two terminals in Godesk to let them talk."
            .to_owned();
    }
    let mut out = String::from("Terminals connected to you (use the name with `godesk send` or `godesk read`):\n");
    for t in reachable {
        if let Some(agent) = reach.agents.agent(&t.id) {
            out.push_str(&format!("  {}  running {} (send it a prompt)\n", t.name, agent.program()));
            continue;
        }
        let program = reach
            .ptys
            .activity(&t.id)
            .program
            .and_then(|argv| argv.first().cloned())
            .map(|p| Path::new(&p).file_stem().map_or(p.clone(), |s| s.to_string_lossy().into_owned()));
        let note = program.map_or_else(|| "  idle shell".to_owned(), |program| format!("  running {program}"));
        out.push_str(&format!("  {}{note}\n", t.name));
    }
    out
}

/// Body: the target's name on the first line, the message after it.
fn send(shared: &Shared, reach: &Reach, from: &str, body: &str) -> Result<String, String> {
    let (to, message) = body.split_once('\n').unwrap_or((body, ""));
    let message = message.trim_end_matches(['\r', '\n']);
    if message.trim().is_empty() {
        return Err("godesk: the message is empty".into());
    }
    let text = typeable(message)?;
    let target = resolve(shared, from, to)?;
    ensure_reachable(shared, from, &target)?;
    if reach.agents.is_open(&target.id) {
        // The terminal shows its agent: the message is the agent's prompt,
        // not something for the shell under it.
        reach
            .agents
            .send(&target.id, &text, reach.agent_sink.clone())
            .map_err(|e| format!("godesk: {} cannot take it now: {e}", target.name))?;
        touch(shared, &target.workspace_id, from, &target.id);
        return Ok(format!("Sent to {}.", target.name));
    }
    reach.ptys.write(&target.id, text.into_bytes()).map_err(|e| format!("godesk: {e}"))?;
    thread::sleep(ENTER_DELAY);
    reach.ptys.write(&target.id, b"\r".to_vec()).map_err(|e| format!("godesk: {e}"))?;
    touch(shared, &target.workspace_id, from, &target.id);
    Ok(format!("Sent to {}.", target.name))
}

/// The text to type into a terminal: no control characters, so a peer
/// cannot send ANSI sequences or Ctrl+C (the rule of
/// [`crate::pty::command_line`]).
fn typeable(message: &str) -> Result<String, String> {
    // A message of several lines is sent as one: Enter inside it would
    // submit the first line alone.
    let text = message.replace("\r\n", "\n").replace('\n', " ");
    if text.chars().any(char::is_control) {
        return Err("godesk: the message holds control characters".into());
    }
    Ok(text)
}

fn read(shared: &Shared, reach: &Reach, from: &str, name: &str, lines: usize) -> Result<String, String> {
    let target = resolve(shared, from, name)?;
    ensure_reachable(shared, from, &target)?;
    // The terminal shows its agent: what it said, not the shell under it.
    if let Some(transcript) = reach.agents.transcript(&target.id, lines) {
        touch(shared, &target.workspace_id, &target.id, from);
        return Ok(format!("--- last messages of {} ---\n{transcript}", target.name));
    }
    let (data, _, _) = reach.ptys.snapshot(&target.id).map_err(|e| format!("godesk: {e}"))?;
    let text = AnsiStripper::default().strip(&String::from_utf8_lossy(&data));
    let kept: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let start = kept.len().saturating_sub(lines.max(1));
    touch(shared, &target.workspace_id, &target.id, from);
    Ok(format!("--- last output of {} ---\n{}", target.name, kept[start..].join("\n")))
}

/// The folder the `z` jump should change to: the best frecency match for the
/// query (an empty query is the single most frecent folder). The jump counts
/// as a visit, so a folder reached often stays near the top.
fn dir(reach: &Reach, query: &str) -> Result<String, String> {
    match reach.dirs.best(query) {
        Some(path) => {
            reach.dirs.add(std::path::Path::new(&path));
            Ok(path)
        }
        None if query.is_empty() => Err("z: no folders learned yet".to_owned()),
        None => Err(format!("z: no folder matches \"{query}\"")),
    }
}

/// Files listed for one base in `godesk kb list` before the rest are summarized.
const KB_LIST_LIMIT: usize = 60;

/// Lists the knowledge bases an arrow connects to the caller, each with a
/// compact index of its files. The caller reads a file with `godesk kb read`.
fn kb_list(shared: &Shared, reach: &Reach, from: &str) -> String {
    let bases = shared.model().knowledge_bases_linked_to(from);
    if bases.is_empty() {
        return "No knowledge base is connected to you. In Godesk, draw an arrow from a knowledge-base node to this terminal."
            .to_owned();
    }
    let mut out = String::from("Knowledge bases connected to you (read a file with `godesk kb read <file>`):\n");
    for kb in &bases {
        let index = reach.kb.index(&kb.id, Path::new(&kb.path));
        out.push_str(&format!("\n[{}] — {} file(s):\n", kb.name, index.entries.len()));
        for entry in index.entries.iter().take(KB_LIST_LIMIT) {
            out.push_str(&format!(
                "  {}  ({}, {})\n",
                entry.rel,
                entry.kind.label(),
                crate::knowledge::human_size(entry.size)
            ));
        }
        if index.entries.len() > KB_LIST_LIMIT {
            out.push_str(&format!("  …and {} more file(s)\n", index.entries.len() - KB_LIST_LIMIT));
        }
        // Reading the base lights the arrow that connects it (kb → terminal).
        touch_node(shared, &kb.id, from);
    }
    out
}

/// Body: the base's name on the first line (may be empty), the file on the next.
fn kb_read(shared: &Shared, reach: &Reach, from: &str, body: &str) -> Result<String, String> {
    let (name, file) = body.split_once('\n').unwrap_or(("", body));
    let (name, file) = (name.trim(), file.trim());
    if file.is_empty() {
        return Err("godesk: name a file, as `godesk kb read <file>`".into());
    }
    let bases = shared.model().knowledge_bases_linked_to(from);
    if bases.is_empty() {
        return Err("godesk: no knowledge base is connected to you".into());
    }
    let kb = if name.is_empty() {
        match bases.as_slice() {
            [only] => only.clone(),
            _ => {
                let names: Vec<&str> = bases.iter().map(|k| k.name.as_str()).collect();
                return Err(format!(
                    "godesk: several knowledge bases are connected ({}). Use `godesk kb read <base> <file>`.",
                    names.join(", ")
                ));
            }
        }
    } else {
        let lower = name.to_lowercase();
        let matches: Vec<_> = bases.iter().filter(|k| k.name.to_lowercase() == lower || k.id == name).collect();
        match matches.as_slice() {
            [only] => (*only).clone(),
            [] => return Err(format!("godesk: no connected knowledge base is called \"{name}\"")),
            _ => return Err(format!("godesk: \"{name}\" names several connected knowledge bases")),
        }
    };
    let text = reach.kb.read(&kb.id, Path::new(&kb.path), file)?;
    touch_node(shared, &kb.id, from);
    Ok(format!("--- {} / {} ---\n{}", kb.name, file, text))
}

/// Most matches returned by `godesk kb search` before it stops.
const KB_SEARCH_LIMIT: usize = 50;

/// Searches the readable documents of the connected knowledge bases for `query`
/// (case-insensitive), returning `base / file:line: text` for each match.
/// Images are skipped (reading one runs OCR); the first search over PDFs and
/// Office files extracts them, and later searches reuse the cache.
fn kb_search(shared: &Shared, reach: &Reach, from: &str, query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("godesk: search for what? Use `godesk kb search <text>`".into());
    }
    let bases = shared.model().knowledge_bases_linked_to(from);
    if bases.is_empty() {
        return Err("godesk: no knowledge base is connected to you".into());
    }
    let mut out = String::new();
    let mut hits = 0;
    'bases: for kb in &bases {
        let root = Path::new(&kb.path);
        let entries = reach.kb.index(&kb.id, root).entries.clone();
        let remaining = KB_SEARCH_LIMIT - hits;
        match reach.kb.search_text(&kb.id, root, &entries, query, remaining) {
            Ok(matches) => {
                for hit in matches {
                    out.push_str(&format!("{} / {}:{}: {}\n", kb.name, hit.rel, hit.line, hit.snippet));
                    hits += 1;
                    if hits >= KB_SEARCH_LIMIT {
                        out.push_str("…more matches not shown\n");
                        touch_node(shared, &kb.id, from);
                        break 'bases;
                    }
                }
            }
            // One base's index failing must not sink the whole search.
            Err(err) => out.push_str(&format!("{}: {err}\n", kb.name)),
        }
        touch_node(shared, &kb.id, from);
    }
    if hits == 0 {
        Ok(format!("No matches for \"{query}\" in the connected knowledge bases."))
    } else {
        Ok(format!("Matches for \"{query}\":\n{out}"))
    }
}

const HELP: &str = "godesk: talk to the other terminals of Godesk.
  godesk list                         the terminals and what runs in them
  godesk send <terminal> <message>    type a message into a terminal and press Enter
  godesk read <terminal> [lines]      print the end of a terminal's output
  godesk kb                           the knowledge bases connected to you and their files
  godesk kb read [<base>] <file>      print a document from a connected knowledge base
  godesk kb search <text>             find text across the connected knowledge bases";

/// The `godesk` command for POSIX shells (Git Bash, which Claude Code uses
/// on Windows; sh/bash/zsh elsewhere), using curl.
///
/// The port and token are baked in rather than read from the environment: they
/// change on every restart, but a terminal that was open before the restart
/// keeps the old values in its own environment. The script is rewritten with the
/// current endpoint each start, so running it always reaches the live bridge.
/// Only the terminal id (stable per terminal) is still read from the
/// environment, and its presence is what marks a real Godesk terminal.
fn sh_command(port: u16, token: &str) -> String {
    format!(
        r#"#!/bin/sh
# Written by Godesk at every start with the current endpoint. {HELP_LINE}
if [ -z "$GODESK_TERMINAL_ID" ]; then echo "godesk: not inside a Godesk terminal" >&2; exit 1; fi
url="http://127.0.0.1:{port}"
# --connect-timeout fails fast when Godesk is not listening; --max-time bounds a
# stuck request so it returns a readable error instead of hanging until the
# caller's own timeout.
call() {{ curl -sS --connect-timeout 5 --max-time 115 -H "X-Godesk-Token: {token}" -H "X-Godesk-From: $GODESK_TERMINAL_ID" "$@"; }}
case "$1" in
  list) call "$url/list" ;;
  send)
    [ $# -ge 3 ] || {{ echo "usage: godesk send <terminal> <message>" >&2; exit 2; }}
    to="$2"; shift 2
    {{ printf '%s\n' "$to"; printf '%s' "$*"; }} | call --data-binary @- "$url/send" ;;
  read)
    [ $# -ge 2 ] || {{ echo "usage: godesk read <terminal> [lines]" >&2; exit 2; }}
    printf '%s' "$2" | call -H "X-Godesk-Lines: ${{3:-{DEFAULT_READ_LINES}}}" --data-binary @- "$url/read" ;;
  kb)
    case "$2" in
      read)
        if [ $# -ge 4 ]; then kbname="$3"; file="$4"; else kbname=""; file="$3"; fi
        [ -n "$file" ] || {{ echo "usage: godesk kb read [<base>] <file>" >&2; exit 2; }}
        {{ printf '%s\n' "$kbname"; printf '%s' "$file"; }} | call --data-binary @- "$url/kb/read" ;;
      search)
        shift 2
        [ $# -ge 1 ] || {{ echo "usage: godesk kb search <text>" >&2; exit 2; }}
        printf '%s' "$*" | call --data-binary @- "$url/kb/search" ;;
      list|"") call "$url/kb/list" ;;
      *) echo "usage: godesk kb [list|read <file>|search <text>]" >&2; exit 2 ;;
    esac ;;
  *) cat <<'EOF'
{HELP}
EOF
  ;;
esac
"#,
        HELP_LINE = "Usage: godesk help"
    )
}

/// The `godesk` command for PowerShell and cmd: a .cmd that runs a .ps1,
/// which needs no curl and survives any characters in the message.
#[cfg(windows)]
const CMD_COMMAND: &str = "@echo off\r\nrem Written by Godesk at every start. Usage: godesk help\r\nsetlocal\r\nrem The arguments go through the environment, not through the command line:\r\nrem cmd.exe would expand them again, and PowerShell would read them as code.\r\nset \"GODESK_CLI_ARGS=%*\"\r\npowershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"%~dp0godesk.ps1\"\r\nendlocal\r\n";

#[cfg(windows)]
fn ps1_command(port: u16, token: &str) -> String {
    format!(
        r#"# Written by Godesk at every start with the current endpoint. Run through godesk.cmd.
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArgs)
$help = @"
{HELP}
"@
# The words of the command line: what godesk.cmd put in the environment,
# or what was typed when this script is run directly.
function Split-Words([string]$text) {{
  $words = @()
  $current = ''
  $quoted = $false
  $started = $false
  foreach ($ch in $text.ToCharArray()) {{
    if ($ch -eq '"') {{ $quoted = -not $quoted; $started = $true; continue }}
    if (-not $quoted -and $ch -eq ' ') {{
      if ($started) {{ $words += $current; $current = ''; $started = $false }}
      continue
    }}
    $current += $ch
    $started = $true
  }}
  if ($started) {{ $words += $current }}
  return $words
}}
if (-not $env:GODESK_TERMINAL_ID) {{ [Console]::Error.WriteLine('godesk: not inside a Godesk terminal'); exit 1 }}
# The @() around the whole expression matters: PowerShell unrolls the array
# an if-statement returns, and a single word would end up as a string whose
# first letter is the command.
$words = @($(if ($env:GODESK_CLI_ARGS) {{ Split-Words $env:GODESK_CLI_ARGS }} else {{ $CliArgs }}) | Where-Object {{ $null -ne $_ }})
$command = if ($words.Count -gt 0) {{ $words[0] }} else {{ '' }}
$target = if ($words.Count -gt 1) {{ $words[1] }} else {{ '' }}
$rest = @(if ($words.Count -gt 2) {{ $words[2..($words.Count - 1)] }})
$headers = @{{ 'X-Godesk-Token' = '{token}'; 'X-Godesk-From' = $env:GODESK_TERMINAL_ID }}
$url = "http://127.0.0.1:{port}"
# WebClient, not Invoke-WebRequest: Windows PowerShell 5.1 sends a byte[]
# body as its decimal text.
[Net.ServicePointManager]::Expect100Continue = $false
function Call([string]$Method, [string]$Path, [string]$Body) {{
  $client = New-Object Net.WebClient
  foreach ($name in $headers.Keys) {{ $client.Headers.Add($name, $headers[$name]) }}
  try {{
    $bytes = if ($Method -eq 'GET') {{ $client.DownloadData("$url$Path") }} else {{
      $client.Headers.Add('Content-Type', 'text/plain; charset=utf-8')
      $client.UploadData("$url$Path", 'POST', [Text.Encoding]::UTF8.GetBytes($Body))
    }}
    [Console]::Out.Write([Text.Encoding]::UTF8.GetString($bytes))
  }} catch [Net.WebException] {{
    $response = $_.Exception.Response
    if ($response) {{ [Console]::Error.Write((New-Object IO.StreamReader($response.GetResponseStream())).ReadToEnd()) }} else {{ [Console]::Error.WriteLine("godesk: $($_.Exception.Message)") }}
    exit 1
  }}
}}
switch ($command) {{
  'list' {{ Call 'GET' '/list' $null }}
  'send' {{
    if (-not $target -or -not $rest) {{ [Console]::Error.WriteLine('usage: godesk send <terminal> <message>'); exit 2 }}
    Call 'POST' '/send' ("$target`n" + ($rest -join ' '))
  }}
  'read' {{
    if (-not $target) {{ [Console]::Error.WriteLine('usage: godesk read <terminal> [lines]'); exit 2 }}
    $lines = if ($rest) {{ $rest[0] }} else {{ '{DEFAULT_READ_LINES}' }}
    $headers['X-Godesk-Lines'] = $lines
    Call 'POST' '/read' $target
  }}
  'kb' {{
    switch ($target) {{
      'read' {{
        if (-not $rest) {{ [Console]::Error.WriteLine('usage: godesk kb read [<base>] <file>'); exit 2 }}
        if ($rest.Count -ge 2) {{ $kbname = $rest[0]; $file = $rest[1] }} else {{ $kbname = ''; $file = $rest[0] }}
        Call 'POST' '/kb/read' ("$kbname`n" + $file)
      }}
      'search' {{
        if (-not $rest) {{ [Console]::Error.WriteLine('usage: godesk kb search <text>'); exit 2 }}
        Call 'POST' '/kb/search' ($rest -join ' ')
      }}
      default {{ Call 'GET' '/kb/list' $null }}
    }}
  }}
  default {{ $help }}
}}
"#
    )
}

/// Writes the `godesk` command scripts, baking in the current port and token.
/// The scripts are rewritten on every start, and every terminal runs this one
/// shared copy, so a terminal that outlived a restart reaches the new endpoint
/// even though its own environment still holds the old port (see [`sh_command`]).
fn write_commands(bin_dir: &Path, port: u16, token: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(bin_dir)?;
    let sh = bin_dir.join("godesk");
    std::fs::write(&sh, sh_command(port, token))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        // `godesk.ps1` is what PowerShell runs (it finds it before the .cmd);
        // the .cmd is for cmd.exe, and it hands the arguments over through
        // the environment.
        std::fs::write(bin_dir.join("godesk.ps1"), ps1_command(port, token))?;
        std::fs::write(bin_dir.join("godesk.cmd"), CMD_COMMAND)?;
    }
    Ok(())
}

/// The parts of the bridge that need no terminal: the answers it gives to
/// a rude or broken client, and the terminals it picks.
/// What the bridge reaches in tests: the shells, and no agent yet.
#[cfg(test)]
fn test_reach(shared: &Arc<Shared>, ptys: PtyManager) -> Reach {
    let agents = AgentSessions::default();
    let agent_sink = Arc::new(crate::core::AgentEvents { shared: shared.clone(), sessions: agents.clone() });
    let dirs = Arc::new(crate::dirs::DirStore::load(std::env::temp_dir().join("godesk-test-dirs.json")));
    let kb = crate::knowledge::KbStore::new(std::env::temp_dir().join("godesk-test-kb"));
    Reach { ptys, agents, agent_sink, dirs, kb }
}

#[cfg(test)]
mod unit_tests {
    use std::sync::Mutex;

    use super::*;
    use crate::core::{CoreSink, Events};
    use crate::model::Rect;
    use crate::persist::Persister;
    use crate::pty::PtyConfig;
    use crate::state::Model;

    struct Quiet;

    impl Events for Quiet {
        fn send(&self, _: &str, _: serde_json::Value) {}
    }

    fn shared(dir: &Path) -> Arc<Shared> {
        let model = Arc::new(Mutex::new(Model::default()));
        Arc::new(Shared::new(Box::new(Quiet), model.clone(), Persister::spawn(dir.join("state.json"), model)))
    }

    /// A connected pair of sockets.
    fn connected() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }


    #[test]
    fn a_client_that_says_nothing_is_answered_instead_of_waited_for() {
        let (_client, server) = connected();
        let error = read_request_in(&server, Duration::from_millis(200)).unwrap_err();
        assert_eq!(error.status, "408 Request Timeout");
    }

    #[test]
    fn a_body_that_is_too_long_is_refused_and_not_truncated() {
        let (mut client, server) = connected();
        client.write_all(b"POST /send HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n").unwrap();
        drop(client);
        let error = read_request_in(&server, Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.status, "413 Payload Too Large");
    }

    #[test]
    fn a_body_of_the_right_size_is_read() {
        let (mut client, server) = connected();
        client.write_all(b"POST /send HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        let request = read_request_in(&server, Duration::from_secs(2)).unwrap();
        assert_eq!((request.method.as_str(), request.path.as_str(), request.body.as_str()), ("POST", "/send", "hello"));
    }

    #[test]
    fn the_token_is_compared_without_giving_away_where_it_differs() {
        assert!(same_secret(b"secret", b"secret"));
        assert!(!same_secret(b"secret", b"secrez"));
        assert!(!same_secret(b"secret", b"secre"));
        assert!(!same_secret(b"", b"secret"));
        assert!(same_secret(b"", b""));
    }

    #[test]
    fn the_sh_command_bakes_in_the_current_endpoint() {
        let script = sh_command(54321, "deadbeefcafe");
        // The live port and token are in the script, not read from the (possibly
        // stale) environment, so a terminal that outlived a restart still works.
        assert!(script.contains("127.0.0.1:54321"), "{script}");
        assert!(script.contains("X-Godesk-Token: deadbeefcafe"), "{script}");
        assert!(!script.contains("$GODESK_PORT"), "the port must not come from the env");
        assert!(!script.contains("$GODESK_TOKEN"), "the token must not come from the env");
        // The terminal id (stable per terminal) still marks a real Godesk shell.
        assert!(script.contains("$GODESK_TERMINAL_ID"), "{script}");
    }

    #[test]
    fn a_name_that_fits_several_terminals_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(dir.path());
        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };
        let (one, two) = {
            let mut model = shared.model();
            let workspace = model.active_workspace_id().to_owned();
            let one = model.add_terminal(&workspace, rect).unwrap().id;
            let two = model.add_terminal(&workspace, rect).unwrap().id;
            model.rename_terminal(&one, "coder").unwrap();
            model.rename_terminal(&two, "coder").unwrap();
            (one, two)
        };
        let error = resolve(&shared, &one, "coder").unwrap_err();
        assert!(error.contains("several"), "{error}");
        assert!(error.contains("coder"), "{error}");
        // The id of a terminal is never ambiguous.
        assert!(resolve(&shared, &one, &two).is_ok());
    }

    #[test]
    fn a_message_with_control_characters_is_refused() {
        assert_eq!(typeable("hello world").unwrap(), "hello world");
        assert_eq!(typeable("one\ntwo").unwrap(), "one two");
        for message in ["\x1b[31mred", "a\x03b", "a\rb", "a\tb"] {
            let error = typeable(message).unwrap_err();
            assert!(error.contains("control characters"), "{error}");
        }
    }

    #[test]
    fn kb_commands_obey_the_arrow() {
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(dir.path());
        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };
        // A knowledge-base folder with one document.
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("a.txt"), "hello kb").unwrap();
        let (terminal, kb) = {
            let mut m = shared.model();
            let ws = m.active_workspace_id().to_owned();
            let terminal = m.add_terminal(&ws, rect).unwrap().id;
            let kb = m.add_knowledge_base(&ws, "docs", docs.to_str().unwrap(), rect).unwrap().id;
            (terminal, kb)
        };
        let ptys = PtyManager::new(
            PtyConfig { home: dir.path().to_owned(), ..PtyConfig::default() },
            Arc::new(CoreSink(shared.clone())),
        );
        let reach = test_reach(&shared, ptys);

        // Without an arrow the base is invisible and unreadable.
        assert!(kb_list(&shared, &reach, &terminal).contains("No knowledge base"));
        assert!(kb_read(&shared, &reach, &terminal, "\na.txt").is_err());

        // The arrow opens it.
        shared.model().add_manual_link(&kb, &terminal, 0).unwrap();
        let listed = kb_list(&shared, &reach, &terminal);
        assert!(listed.contains("a.txt") && listed.contains("docs"), "{listed}");
        assert!(kb_read(&shared, &reach, &terminal, "\na.txt").unwrap().contains("hello kb"));
        // Search reaches it too. Each of list, read and search lights the arrow
        // through `touch_node`; a regression that held the model lock across that
        // call (an `if let` over `shared.model()`) would deadlock this line.
        assert!(kb_search(&shared, &reach, &terminal, "hello").unwrap().contains("a.txt"));
        // An escaping path is still refused even with the arrow.
        assert!(kb_read(&shared, &reach, &terminal, "\n../secret").is_err());
    }

    #[test]
    fn too_many_connections_at_once_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(dir.path());
        let ptys = PtyManager::new(
            PtyConfig { home: dir.path().to_owned(), ..PtyConfig::default() },
            Arc::new(CoreSink(shared.clone())),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        serve_with_limit(listener, "token".to_owned(), shared.clone(), test_reach(&shared, ptys), 2);
        // Two clients that say nothing hold the two slots.
        let mut held = Vec::new();
        for _ in 0..2 {
            held.push(TcpStream::connect(address).unwrap());
        }
        thread::sleep(Duration::from_millis(300));
        // The next one is told the bridge is full.
        let mut refused = TcpStream::connect(address).unwrap();
        refused.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut answer = String::new();
        refused.read_to_string(&mut answer).unwrap();
        assert!(answer.contains("503"), "{answer}");
    }
}

/// Two real Windows shells: one reaches the other with the `godesk` command,
/// from PowerShell and from Git Bash, but only while an arrow connects them.
#[cfg(all(test, windows))]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use super::*;
    use crate::core::{CoreSink, Events};
    use crate::model::{LinkKind, Rect};
    use crate::persist::Persister;
    use crate::pty::{PtyConfig, SpawnRequest};
    use crate::state::Model;

    struct Quiet;

    impl Events for Quiet {
        fn send(&self, _: &str, _: serde_json::Value) {}
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn output(ptys: &PtyManager, id: &str) -> String {
        let text = String::from_utf8_lossy(&ptys.snapshot(id).unwrap().0).into_owned();
        AnsiStripper::default().strip(&text)
    }

    #[test]
    fn an_arrow_gates_who_the_terminals_can_reach() {
        let dir = tempfile::tempdir().unwrap();
        let model = Arc::new(Mutex::new(Model::default()));
        let shared =
            Arc::new(Shared::new(Box::new(Quiet), model.clone(), Persister::spawn(dir.path().join("state.json"), model)));
        let (listener, env) = bind(dir.path()).unwrap();
        let config = PtyConfig {
            home: dir.path().to_owned(),
            env: env.vars.clone(),
            path_prefix: Some(env.bin_dir.clone()),
            ..PtyConfig::default()
        };
        let ptys = PtyManager::new(config, Arc::new(CoreSink(shared.clone())));
        let token = env.vars.iter().find(|(k, _)| k == "GODESK_TOKEN").unwrap().1.clone();
        let reach = test_reach(&shared, ptys.clone());
        serve(listener, token, shared.clone(), reach.clone());

        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };
        let (ws, one, two, three) = {
            let mut m = shared.model();
            let ws = m.active_workspace_id().to_owned();
            let one = m.add_terminal(&ws, rect).unwrap();
            let two = m.add_terminal(&ws, rect).unwrap();
            let three = m.add_terminal(&ws, rect).unwrap();
            // Only Terminal 1 and Terminal 2 are connected by an arrow.
            m.add_manual_link(&one.id, &two.id, 1).unwrap();
            (ws, one, two, three)
        };
        assert_eq!((one.name.as_str(), two.name.as_str(), three.name.as_str()), ("Terminal 1", "Terminal 2", "Terminal 3"));
        for t in [&one, &two, &three] {
            let req = SpawnRequest {
                terminal_id: &t.id,
                terminal_name: &t.name,
                workspace_id: &ws,
                cwd: None,
                history: None,
                cols: 120,
                rows: 30,
            };
            ptys.spawn(&req).unwrap();
            // ConPTY waits for the cursor position, which xterm.js answers in the app.
            let asked = || String::from_utf8_lossy(&ptys.snapshot(&t.id).unwrap().0).contains("\x1b[6n");
            assert!(eventually(asked));
            ptys.write(&t.id, b"\x1b[1;1R".to_vec()).unwrap();
        }
        // The arrow between one and two pulses once traffic flows over it.
        let active = |a: &str, b: &str| {
            shared.model().links_of(&ws).iter().any(|l| {
                l.kind == LinkKind::Manual
                    && ((l.source_id == a && l.target_id == b) || (l.source_id == b && l.target_id == a))
                    && l.last_activity_at > 1
            })
        };

        // `godesk list` shows only the terminals connected to the caller.
        ptys.write(&one.id, b"godesk list\r".to_vec()).unwrap();
        assert!(eventually(|| output(&ptys, &one.id).contains("Terminal 2  idle shell")), "{}", output(&ptys, &one.id));
        assert!(!output(&ptys, &one.id).contains("Terminal 3"), "an unconnected terminal is not listed");

        ptys.write(&one.id, "godesk send 'Terminal 2' 'hello world & ñandú'\r".as_bytes().to_vec()).unwrap();
        assert!(eventually(|| output(&ptys, &one.id).contains("Sent to Terminal 2.")), "{}", output(&ptys, &one.id));
        assert!(eventually(|| output(&ptys, &two.id).contains("hello world & ñandú")), "{}", output(&ptys, &two.id));
        assert!(eventually(|| active(&one.id, &two.id)), "the arrow lights up with traffic");

        ptys.write(&one.id, b"godesk read 'terminal 2' 5\r".to_vec()).unwrap();
        assert!(eventually(|| output(&ptys, &one.id).contains("--- last output of Terminal 2 ---")));

        // With no arrow to Terminal 3, Terminal 1 cannot reach it, and the
        // shell under Terminal 3 is never written to.
        ptys.write(&one.id, b"godesk send 'Terminal 3' 'should be refused'\r".to_vec()).unwrap();
        assert!(eventually(|| output(&ptys, &one.id).contains("no arrow connects")), "{}", output(&ptys, &one.id));
        assert!(!output(&ptys, &three.id).contains("should be refused"));

        // Git Bash, which Claude Code runs its commands in on Windows.
        let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        if bash.is_file() {
            // Single quotes inside: PowerShell 5.1 mangles nested double quotes.
            let line = format!("& '{}' -c \"godesk send 'Terminal 2' from bash\"\r", bash.display());
            ptys.write(&one.id, line.into_bytes()).unwrap();
            assert!(eventually(|| output(&ptys, &two.id).contains("from bash")), "{}", output(&ptys, &one.id));
        }

        // The `z` jump cd's to the best folder Godesk learned for the query.
        let target = dir.path().join("zoxide-target");
        std::fs::create_dir_all(&target).unwrap();
        reach.dirs.add(&target);
        ptys.write(&one.id, b"z zoxide\r".to_vec()).unwrap();
        ptys.write(&one.id, b"[Console]::Out.Write('AT=' + (Get-Location).Path)\r".to_vec()).unwrap();
        assert!(
            eventually(|| output(&ptys, &one.id).to_lowercase().contains("at=") && output(&ptys, &one.id).to_lowercase().contains("zoxide-target")),
            "{}",
            output(&ptys, &one.id)
        );

        // Terminal 2 now shows an agent: a message is its prompt, not
        // something typed into the shell under it, and a read gives what
        // the agent said.
        reach.agents.open_idle(&two.id, crate::agent_terminal::AgentKind::Claude);
        reach.agents.remember(&two.id, r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hola desde claude"}]}}"#);
        ptys.write(&one.id, b"godesk list\r".to_vec()).unwrap();
        assert!(eventually(|| output(&ptys, &one.id).contains("Terminal 2  running claude")), "{}", output(&ptys, &one.id));
        ptys.write(&one.id, b"godesk read 'Terminal 2'\r".to_vec()).unwrap();
        assert!(eventually(|| output(&ptys, &one.id).contains("hola desde claude")), "{}", output(&ptys, &one.id));
        ptys.write(&one.id, b"godesk send 'Terminal 2' 'para el agente'\r".to_vec()).unwrap();
        // The agent was given the prompt (a stand-in that cannot run it
        // says so), and the shell under it got nothing.
        assert!(
            eventually(|| reach.agents.history(&two.id).0.contains("para el agente")),
            "{}",
            output(&ptys, &one.id)
        );
        assert!(!output(&ptys, &two.id).contains("para el agente"));
        reach.agents.close_all();
        ptys.close_all();
    }
}
