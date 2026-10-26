//! The coding agents Godesk draws in a terminal.
//!
//! When a shell starts an agent Godesk knows (`claude`, `codex`, `opencode`,
//! `mimo`), the terminal shows Godesk's view of it instead of the agent's own
//! screen: the agent runs in its machine-readable mode (one JSON event per
//! line), and the frontend draws the conversation from those events.
//!
//! Agents speak in one of two ways:
//! - Claude Code keeps one process for the whole conversation: it reads one
//!   JSON message per line on its input (`--input-format stream-json`).
//! - Codex, OpenCode and MiMo (an OpenCode build) answer one prompt per
//!   process. Each turn starts the agent again, on the session the first turn
//!   opened, with the prompt on its input (never on the command line, so no
//!   quoting can break it).

use serde_json::Value;

/// The coding agents Godesk can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    OpenCode,
    /// MiMo Code: OpenCode under another name, with the same protocol.
    Mimo,
}

/// How the user started the agent in the shell, kept for the agent's
/// session: the conversation to continue, the model, and a first prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Launch {
    /// The session to continue (`claude --resume <id>`, `codex resume <id>`).
    pub session: Option<String>,
    /// Continue the last conversation in the folder (`--continue`, `--last`).
    pub continue_last: bool,
    pub model: Option<String>,
    /// Claude Code only: `--dangerously-skip-permissions`.
    pub skip_permissions: bool,
    /// Written after the command (`claude "fix the build"`).
    pub prompt: Option<String>,
    /// The program the shell ran (`C:\…\claude.exe`, or `node …\cli.js`),
    /// to start the same one even where Godesk's own PATH does not find it.
    pub program: Option<Vec<String>>,
}

impl AgentKind {
    /// The agent called `name` ("claude", "codex", "opencode", "mimo").
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "mimo" => Some(Self::Mimo),
            _ => None,
        }
    }

    /// Whether one process serves the whole conversation.
    pub fn persistent(self) -> bool {
        self == Self::Claude
    }

    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Mimo => "mimo",
        }
    }

    /// The arguments that start the agent: the whole conversation for a
    /// persistent agent, one turn otherwise. `session` is the conversation
    /// to continue; without it, `launch` says whether to continue the last.
    pub fn args(self, session: Option<&str>, launch: &Launch) -> Vec<String> {
        let session = session.filter(|id| safe_id(id));
        let model = launch.model.as_deref().filter(|m| safe_model(m));
        let args: Vec<&str> = match self {
            Self::Claude => {
                let mut args = vec![
                    "--print",
                    "--output-format",
                    "stream-json",
                    "--input-format",
                    "stream-json",
                    "--verbose",
                    // Text arrives as it is written, not once the message is done.
                    "--include-partial-messages",
                ];
                if launch.skip_permissions {
                    args.push("--dangerously-skip-permissions");
                } else {
                    // Edits are taken without asking; anything else that
                    // needs a permission is asked on the output (see
                    // `permission_in`) and the view asks the user. The
                    // `godesk` command is Godesk's own: always allowed.
                    args.extend([
                        "--permission-mode",
                        "acceptEdits",
                        "--permission-prompt-tool",
                        "stdio",
                        "--allowedTools",
                        "Bash(godesk:*)",
                        "PowerShell(godesk:*)",
                    ]);
                }
                // Started again after it stopped: the conversation goes on.
                match session {
                    Some(id) => args.extend(["--resume", id]),
                    None if launch.continue_last => args.push("--continue"),
                    None => {}
                }
                if let Some(model) = model {
                    args.extend(["--model", model]);
                }
                args
            }
            // `-` reads the prompt from the input. The sandbox lets the agent
            // write in its folder, as it would in its own terminal.
            Self::Codex => {
                let mut args = vec!["exec", "--json", "--skip-git-repo-check", "--sandbox", "workspace-write"];
                if let Some(model) = model {
                    args.extend(["--model", model]);
                }
                match session {
                    Some(id) => args.extend(["resume", id]),
                    None if launch.continue_last => args.extend(["resume", "--last"]),
                    None => {}
                }
                args.push("-");
                args
            }
            Self::OpenCode | Self::Mimo => {
                let mut args = vec!["run", "--format", "json"];
                match session {
                    Some(id) => args.extend(["--session", id]),
                    None if launch.continue_last => args.push("--continue"),
                    None => {}
                }
                if let Some(model) = model {
                    args.extend(["--model", model]);
                }
                args
            }
        };
        args.into_iter().map(str::to_owned).collect()
    }

    /// What goes on the agent's input for a message of the user.
    pub fn message(self, text: &str) -> String {
        match self {
            Self::Claude => {
                serde_json::json!({ "type": "user", "message": { "role": "user", "content": text } }).to_string()
            }
            _ => text.to_owned(),
        }
    }

    /// What asks a persistent agent to stop the turn it is working on.
    pub fn interrupt_message(self) -> Option<String> {
        match self {
            Self::Claude => Some(
                serde_json::json!({
                    "type": "control_request",
                    "request_id": format!("godesk-{}", uuid::Uuid::new_v4()),
                    "request": { "subtype": "interrupt" }
                })
                .to_string(),
            ),
            _ => None,
        }
    }

    /// A permission the agent asks for and waits on: its request id, and
    /// what the tool would do with the rules that would allow it from now
    /// on (Claude Code's `can_use_tool` control request).
    pub fn permission_in(self, line: &str) -> Option<(String, (Value, Value))> {
        if self != Self::Claude || !line.contains("can_use_tool") {
            return None;
        }
        let event: Value = serde_json::from_str(line.get(line.find('{')?..)?).ok()?;
        let request = &event["request"];
        if event["type"] != "control_request" || request["subtype"] != "can_use_tool" {
            return None;
        }
        let id = event["request_id"].as_str()?.to_owned();
        Some((id, (request["input"].clone(), request["permission_suggestions"].clone())))
    }

    /// The session a line of the agent's output names, to continue it on
    /// the next turn.
    pub fn session_in(self, line: &str) -> Option<String> {
        let start = line.find('{')?;
        let event: Value = serde_json::from_str(&line[start..]).ok()?;
        let id = match self {
            Self::Claude => event["session_id"].as_str(),
            Self::Codex => (event["type"] == "thread.started").then(|| event["thread_id"].as_str()).flatten(),
            Self::OpenCode | Self::Mimo => event["sessionID"].as_str(),
        }?;
        safe_id(id).then(|| id.to_owned())
    }
}

/// The command that opens `agent` on its own screen, in the shell, with
/// what Godesk's view had: the same program, conversation, model and
/// permissions.
pub fn tui_argv(agent: AgentKind, launch: &Launch, session: Option<&str>) -> Vec<String> {
    let mut argv = launch
        .program
        .clone()
        .filter(|p| p.first().is_some_and(|f| std::path::Path::new(f).is_file()))
        .unwrap_or_else(|| vec![agent.program().to_owned()]);
    let session = session.filter(|id| safe_id(id));
    let model = launch.model.as_deref().filter(|m| safe_model(m));
    let mut add = |args: &[&str]| argv.extend(args.iter().map(|a| (*a).to_owned()));
    match agent {
        AgentKind::Claude => {
            if let Some(id) = session {
                add(&["--resume", id]);
            }
            if let Some(model) = model {
                add(&["--model", model]);
            }
            if launch.skip_permissions {
                add(&["--dangerously-skip-permissions"]);
            }
        }
        AgentKind::Codex => {
            if let Some(model) = model {
                add(&["-m", model]);
            }
            if let Some(id) = session {
                add(&["resume", id]);
            }
        }
        AgentKind::OpenCode | AgentKind::Mimo => {
            if let Some(id) = session {
                add(&["-s", id]);
            }
            if let Some(model) = model {
                add(&["-m", model]);
            }
        }
    }
    argv
}

/// What a line of the agent's stream says, as plain text for someone
/// reading the conversation (`godesk read`, link detection): the user's
/// prompts, the agent's finished messages and the commands it runs.
/// Partial messages, tokens and other bookkeeping say nothing.
pub fn transcript_line(agent: AgentKind, line: &str) -> Option<String> {
    let event: Value = serde_json::from_str(line.get(line.find('{')?..)?).ok()?;
    let text = |value: &Value| value.as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
    let command = |name: &str, input: &Value| {
        let detail = ["command", "file_path", "filePath", "pattern", "path", "url", "query", "description"]
            .iter()
            .find_map(|field| input[field].as_str())
            .unwrap_or_default();
        format!("[{name}] {}", detail.lines().next().unwrap_or_default())
    };
    if event["type"] == "godesk.prompt" {
        return text(&event["text"]).map(|t| format!("> {t}"));
    }
    match agent {
        AgentKind::Claude if event["type"] == "assistant" && event["parent_tool_use_id"].is_null() => {
            let parts: Vec<String> = event["message"]["content"]
                .as_array()?
                .iter()
                .filter_map(|block| match block["type"].as_str()? {
                    "text" => text(&block["text"]),
                    "tool_use" => Some(command(block["name"].as_str().unwrap_or("tool"), &block["input"])),
                    _ => None,
                })
                .collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        AgentKind::Codex if event["type"] == "item.completed" => {
            let item = &event["item"];
            match item["type"].as_str()? {
                "agent_message" => text(&item["text"]),
                "command_execution" => Some(command("shell", item)),
                _ => None,
            }
        }
        AgentKind::OpenCode | AgentKind::Mimo => {
            let part = &event["part"];
            match event["type"].as_str()? {
                "text" => text(&part["text"]),
                "tool_use" => Some(command(part["tool"].as_str().unwrap_or("tool"), &part["state"]["input"])),
                _ => None,
            }
        }
        _ => None,
    }
}

/// What answers a permission request: allow `input` once, allow it and
/// add `rules` (as the agent suggested), or deny it.
pub fn permission_answer(request_id: &str, decision: &str, input: Value, rules: Value) -> Option<String> {
    let response = match decision {
        "allow" => serde_json::json!({ "behavior": "allow", "updatedInput": input }),
        "always" => serde_json::json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": if rules.is_array() { rules } else { serde_json::json!([]) },
        }),
        "deny" => serde_json::json!({ "behavior": "deny", "message": "The user denied this in Godesk." }),
        _ => return None,
    };
    Some(
        serde_json::json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response }
        })
        .to_string(),
    )
}

/// The agent a shell runs and how it was started, when it is an agent
/// Godesk can draw and the command only opens a conversation: `claude`,
/// `claude --resume <id>`, `codex -m gpt-6`, `opencode --continue`,
/// `claude "fix the build"`. Anything else (`claude -p`, `claude mcp list`,
/// `codex exec`, an option Godesk does not know) runs as it is.
pub fn launch_of(argv: &[String]) -> Option<(AgentKind, Launch)> {
    let agent = AgentKind::from_name(crate::resume::agent_of(argv)?)?;
    let (program, args) = argv.split_at(crate::resume::program_len(argv));
    let mut launch = Launch::default();
    // Only a program named by its full path is started again as it is; a
    // bare name is looked up on PATH.
    if program.iter().all(|p| std::path::Path::new(p).is_absolute() && std::path::Path::new(p).is_file()) {
        launch.program = Some(program.to_vec());
    }
    let mut positional = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        // An option's value: after `=`, or the next argument. An optional
        // one is not taken from an argument that is itself an option.
        let mut value = |optional: bool| -> Option<String> {
            if inline.is_some() {
                return inline.clone();
            }
            if optional && iter.peek().is_none_or(|next| next.starts_with('-')) {
                return None;
            }
            iter.next().cloned()
        };
        let opencode = matches!(agent, AgentKind::OpenCode | AgentKind::Mimo);
        match name {
            "-c" | "--continue" if agent != AgentKind::Codex => launch.continue_last = true,
            "-r" | "--resume" if agent == AgentKind::Claude => match value(true) {
                Some(id) => launch.session = Some(id),
                // The list to pick from is the agent's own screen: the last
                // conversation instead.
                None => launch.continue_last = true,
            },
            "-s" | "--session" if opencode => launch.session = Some(value(false)?),
            // `codex resume --last`.
            "--last" if agent == AgentKind::Codex => launch.continue_last = true,
            "--model" => launch.model = Some(value(false)?),
            "-m" if agent != AgentKind::Claude => launch.model = Some(value(false)?),
            "--dangerously-skip-permissions" if agent == AgentKind::Claude => launch.skip_permissions = true,
            flag if flag.starts_with('-') => return None,
            _ => positional.push(arg.clone()),
        }
    }
    match positional.as_slice() {
        [] => {}
        [resume, rest @ ..] if agent == AgentKind::Codex && resume == "resume" => match rest {
            [] => launch.continue_last = true,
            [id] => launch.session = Some(id.clone()),
            _ => return None,
        },
        // A prompt has words; a single word is a subcommand (`claude update`).
        [prompt] if matches!(agent, AgentKind::Claude | AgentKind::Codex) && prompt.trim().contains(' ') => {
            launch.prompt = Some(prompt.trim().to_owned());
        }
        _ => return None,
    }
    let unsafe_session = launch.session.as_deref().is_some_and(|id| !safe_id(id));
    let unsafe_model = launch.model.as_deref().is_some_and(|m| !safe_model(m));
    (!unsafe_session && !unsafe_model).then_some((agent, launch))
}

/// A session id goes on a command line: only letters, digits, `-` and `_`.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A model name goes on a command line too: `provider/model-1.5`, one word.
fn safe_model(model: &str) -> bool {
    !model.is_empty() && model.len() <= 128 && model.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:@".contains(&b))
}

/// The program and arguments that start `agent`.
pub fn argv(agent: AgentKind, session: Option<&str>, launch: &Launch) -> Vec<String> {
    let mut argv = match &launch.program {
        Some(program) if program.first().is_some_and(|p| std::path::Path::new(p).is_file()) => program.clone(),
        _ => launcher(agent.program()),
    };
    argv.extend(agent.args(session, launch));
    argv
}

/// How to start `program`. On Windows an agent installed with npm is a
/// `.cmd` script, which only `cmd` runs; the arguments Godesk passes it are
/// fixed words and checked ids and model names, so `cmd` reads them as they
/// are.
fn launcher(program: &str) -> Vec<String> {
    #[cfg(windows)]
    if let Some(path) = find_on_path(program) {
        let is_script = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        let path = path.to_string_lossy().into_owned();
        return if is_script { vec!["cmd".to_owned(), "/C".to_owned(), path] } else { vec![path] };
    }
    vec![program.to_owned()]
}

/// Whether `agent` can be started on this machine, as `launch` says.
pub fn installed(agent: AgentKind, launch: &Launch) -> bool {
    launch.program.as_ref().and_then(|p| p.first()).is_some_and(|p| std::path::Path::new(p).is_file())
        || find_on_path(agent.program()).is_some()
}

/// The file `program` names on `PATH`, trying each of `PATHEXT` on Windows.
fn find_on_path(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        extensions.iter().map(|ext| dir.join(format!("{program}{ext}"))).find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn each_agent_continues_its_own_session() {
        let none = Launch::default();
        assert_eq!(AgentKind::Codex.args(None, &none).last().map(String::as_str), Some("-"));
        let resumed = AgentKind::Codex.args(Some("01a1-83c1"), &none);
        assert_eq!(&resumed[resumed.len() - 3..], ["resume", "01a1-83c1", "-"]);
        assert_eq!(
            AgentKind::OpenCode.args(Some("ses_e9e7"), &none),
            ["run", "--format", "json", "--session", "ses_e9e7"]
        );
        assert_eq!(AgentKind::Mimo.args(None, &none), ["run", "--format", "json"]);
        // A session id that could be read as more than a word is left out.
        assert!(!AgentKind::OpenCode.args(Some("x & del *"), &none).contains(&"--session".to_owned()));
    }

    #[test]
    fn the_launch_carries_over_to_the_agent() {
        let last = Launch { continue_last: true, model: Some("gpt-6-luna".into()), ..Launch::default() };
        let codex = AgentKind::Codex.args(None, &last);
        assert_eq!(&codex[codex.len() - 5..], ["--model", "gpt-6-luna", "resume", "--last", "-"]);
        // A known session wins over "the last one".
        assert!(AgentKind::Claude.args(Some("a443"), &last).windows(2).any(|w| w == ["--resume", "a443"]));
        assert!(!AgentKind::Claude.args(Some("a443"), &last).contains(&"--continue".to_owned()));
        let skip = Launch { skip_permissions: true, ..Launch::default() };
        assert!(!AgentKind::Claude.args(None, &skip).contains(&"acceptEdits".to_owned()));
    }

    #[test]
    fn knows_which_commands_only_open_a_conversation() {
        let (agent, launch) = launch_of(&words("claude")).unwrap();
        assert_eq!((agent, launch), (AgentKind::Claude, Launch::default()));
        // The program the shell ran is started again, when it is a file.
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("claude.exe");
        std::fs::write(&exe, b"").unwrap();
        let exe = exe.to_string_lossy().into_owned();
        let (_, launch) = launch_of(&[exe.clone(), "-c".to_owned()]).unwrap();
        assert_eq!(launch.program, Some(vec![exe]));
        assert!(launch.continue_last);
        let (_, launch) = launch_of(&words(r"C:\Users\me\.local\bin\claude.exe --resume a443-c019")).unwrap();
        assert_eq!(launch.session.as_deref(), Some("a443-c019"));
        assert!(launch_of(&words("claude -r")).unwrap().1.continue_last);
        assert_eq!(launch_of(&words("claude --model=opus")).unwrap().1.model.as_deref(), Some("opus"));
        let prompt = vec!["claude".to_owned(), "fix the build".to_owned()];
        assert_eq!(launch_of(&prompt).unwrap().1.prompt.as_deref(), Some("fix the build"));

        let (agent, launch) = launch_of(&words("codex -m gpt-6-luna resume 01a1-83c1")).unwrap();
        assert_eq!(agent, AgentKind::Codex);
        assert_eq!((launch.model.as_deref(), launch.session.as_deref()), (Some("gpt-6-luna"), Some("01a1-83c1")));
        assert!(launch_of(&words("codex resume --last")).unwrap().1.continue_last);

        let mimo = words(r"C:\nodejs\node.exe C:\npm\node_modules\@mimo-ai\cli\bin\mimo -c");
        assert_eq!(launch_of(&mimo).map(|(a, l)| (a, l.continue_last)), Some((AgentKind::Mimo, true)));
        assert_eq!(launch_of(&words("opencode -s ses_1")).unwrap().1.session.as_deref(), Some("ses_1"));

        // Anything else runs as it is.
        for line in ["claude -p hi", "claude update", "claude mcp list", "codex exec", "opencode serve", "claude --foo", "aider"] {
            assert_eq!(launch_of(&words(line)), None, "{line}");
        }
        assert_eq!(launch_of(&words("claude --resume x&del")), None);
    }

    #[test]
    fn the_own_screen_opens_on_the_same_conversation() {
        let launch = Launch { model: Some("gpt-6-luna".into()), ..Launch::default() };
        assert_eq!(tui_argv(AgentKind::Codex, &launch, Some("01a1")), words("codex -m gpt-6-luna resume 01a1"));
        assert_eq!(tui_argv(AgentKind::Claude, &Launch::default(), Some("a443")), words("claude --resume a443"));
        assert_eq!(tui_argv(AgentKind::Mimo, &Launch::default(), Some("ses_1")), words("mimo -s ses_1"));
        assert_eq!(tui_argv(AgentKind::OpenCode, &Launch::default(), Some("x & del")), words("opencode"));
        // What the shell typed comes back as Godesk's view would read it.
        let (_, back) = launch_of(&tui_argv(AgentKind::Codex, &launch, Some("01a1"))).unwrap();
        assert_eq!((back.session.as_deref(), back.model.as_deref()), (Some("01a1"), Some("gpt-6-luna")));
    }

    #[test]
    fn the_conversation_reads_as_text() {
        let prompt = r#"{"type":"godesk.prompt","text":"hola"}"#;
        assert_eq!(transcript_line(AgentKind::Mimo, prompt).as_deref(), Some("> hola"));
        let claude = r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"text","text":"Voy."},{"type":"tool_use","name":"Bash","input":{"command":"go run main.go"}}]}}"#;
        assert_eq!(transcript_line(AgentKind::Claude, claude).as_deref(), Some("Voy.\n[Bash] go run main.go"));
        let codex = r#"{"type":"item.completed","item":{"type":"agent_message","text":"hecho"}}"#;
        assert_eq!(transcript_line(AgentKind::Codex, codex).as_deref(), Some("hecho"));
        let mimo = r#"{"type":"text","part":{"type":"text","text":"¡Hola!"}}"#;
        assert_eq!(transcript_line(AgentKind::Mimo, mimo).as_deref(), Some("¡Hola!"));
        assert_eq!(transcript_line(AgentKind::Claude, r#"{"type":"stream_event"}"#), None);
    }

    #[test]
    fn a_permission_is_asked_and_answered() {
        let asked = r#"{"type":"control_request","request_id":"d217","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"whoami"},"permission_suggestions":[{"type":"addRules","behavior":"allow"}]}}"#;
        let (id, (input, rules)) = AgentKind::Claude.permission_in(asked).unwrap();
        assert_eq!(id, "d217");
        assert_eq!(input["command"], "whoami");
        assert_eq!(AgentKind::Codex.permission_in(asked), None);

        let always: Value =
            serde_json::from_str(&permission_answer("d217", "always", input.clone(), rules).unwrap()).unwrap();
        assert_eq!(always["response"]["request_id"], "d217");
        assert_eq!(always["response"]["response"]["updatedInput"]["command"], "whoami");
        assert_eq!(always["response"]["response"]["updatedPermissions"][0]["type"], "addRules");
        let deny: Value = serde_json::from_str(&permission_answer("d217", "deny", input.clone(), Value::Null).unwrap()).unwrap();
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(permission_answer("d217", "maybe", input, Value::Null), None);
    }

    #[test]
    fn finds_the_session_in_what_the_agent_writes() {
        let codex = r#"{"type":"thread.started","thread_id":"01a16183-c118"}"#;
        assert_eq!(AgentKind::Codex.session_in(codex).as_deref(), Some("01a16183-c118"));
        assert_eq!(AgentKind::Codex.session_in(r#"{"type":"turn.started"}"#), None);
        let opencode = r#"{"type":"text","sessionID":"ses_e9e7b5","part":{}}"#;
        assert_eq!(AgentKind::OpenCode.session_in(opencode).as_deref(), Some("ses_e9e7b5"));
        assert_eq!(AgentKind::Mimo.session_in(opencode).as_deref(), Some("ses_e9e7b5"));
        let claude = r#"{"type":"system","subtype":"init","session_id":"a443c019-bfeb"}"#;
        assert_eq!(AgentKind::Claude.session_in(claude).as_deref(), Some("a443c019-bfeb"));
        assert_eq!(AgentKind::Claude.session_in("not json"), None);
    }

    #[test]
    fn a_message_is_what_each_agent_reads() {
        let claude: Value = serde_json::from_str(&AgentKind::Claude.message("hola")).unwrap();
        assert_eq!(claude["message"]["content"], "hola");
        assert_eq!(AgentKind::Codex.message("di \"hola\" & adiós"), "di \"hola\" & adiós");
        assert!(AgentKind::Claude.interrupt_message().unwrap().contains("interrupt"));
        assert!(AgentKind::OpenCode.interrupt_message().is_none());
    }
}
