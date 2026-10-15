use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAgent {
    Claude,
    Codex,
}

impl TerminalAgent {
    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAgentSession {
    pub agent: TerminalAgent,
    pub session_id: Option<String>,
    pub working_directory: PathBuf,
}

impl TerminalAgentSession {
    pub fn resume_command(&self) -> String {
        match &self.session_id {
            Some(session_id) => format!(
                "{} {} {}",
                self.agent.program(),
                if self.agent == TerminalAgent::Claude {
                    "--resume"
                } else {
                    "resume"
                },
                match shlex::try_quote(session_id) {
                    Ok(session_id) => session_id,
                    Err(_) => return self.agent.program().to_owned(),
                },
            ),
            None => self.agent.program().to_owned(),
        }
    }
}

#[cfg(unix)]
pub use unix::{TerminalAgentIntegration, run_helper_if_requested};

#[cfg(not(unix))]
pub fn run_helper_if_requested() -> Option<i32> {
    None
}

#[cfg(unix)]
mod unix {
    use super::*;
    use anyhow::{Context as _, Result, bail};
    use smol::io::AsyncReadExt as _;
    use std::{
        collections::HashMap,
        fs::{self, OpenOptions},
        io::{Read as _, Write as _},
        os::unix::{
            fs::PermissionsExt as _,
            net::UnixStream,
            process::{CommandExt as _, ExitStatusExt as _},
        },
        path::Path,
        process::{Command, Stdio},
    };
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    const SOCKET: &str = "ZED_TERMINAL_AGENT_SOCKET";
    const HELPER: &str = "ZED_TERMINAL_AGENT_HELPER";
    const OWNER: &str = "ZED_TERMINAL_AGENT_OWNER";
    const LAUNCH: &str = "ZED_TERMINAL_AGENT_LAUNCH";
    const AGENT: &str = "ZED_TERMINAL_AGENT_KIND";
    const HOOK: &str = "if [ -n \"$ZED_TERMINAL_AGENT_SOCKET\" ] && [ -x \"$ZED_TERMINAL_AGENT_HELPER\" ]; then \"$ZED_TERMINAL_AGENT_HELPER\" --terminal-agent-hook {agent}; fi";
    const MAX_EVENT_BYTES: u64 = 64 * 1024;

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "event", rename_all = "snake_case")]
    enum Event {
        Started {
            launch: String,
            session: TerminalAgentSession,
        },
        Session {
            launch: String,
            session_id: String,
        },
        Exited {
            launch: String,
        },
    }

    #[derive(Default)]
    struct SessionState {
        launch: Option<String>,
        session: Option<TerminalAgentSession>,
    }

    impl SessionState {
        fn apply(&mut self, event: Event) -> bool {
            match event {
                Event::Started { launch, session } => {
                    self.launch = Some(launch);
                    self.session = Some(session);
                    true
                }
                Event::Session { launch, session_id } if self.launch.as_ref() == Some(&launch) => {
                    if uuid::Uuid::parse_str(&session_id).is_err() {
                        return false;
                    }
                    let Some(session) = &mut self.session else {
                        return false;
                    };
                    if session.session_id.as_ref() == Some(&session_id) {
                        return false;
                    }
                    session.session_id = Some(session_id);
                    true
                }
                Event::Exited { launch } if self.launch.as_ref() == Some(&launch) => {
                    self.launch = None;
                    self.session = None;
                    true
                }
                _ => false,
            }
        }
    }

    pub struct TerminalAgentIntegration {
        directory: tempfile::TempDir,
        listener: smol::Async<std::os::unix::net::UnixListener>,
        state: SessionState,
    }

    impl TerminalAgentIntegration {
        pub fn new() -> Result<Self> {
            // macOS Unix sockets have a short path limit, independent of TMPDIR.
            let directory = tempfile::Builder::new()
                .prefix("zed-agent-")
                .tempdir_in("/tmp")?;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
            let listener = std::os::unix::net::UnixListener::bind(directory.path().join("events"))?;
            let mut script = String::new();
            let mut fish_script = String::new();
            for agent in [TerminalAgent::Claude, TerminalAgent::Codex] {
                let program = agent.program();
                script.push_str(&format!(r#"
if [ -n "${{ZSH_VERSION-}}" ]; then
    if (( ! $+functions[{program}] && ! $+aliases[{program}] )); then
        function {program} {{ "$ZED_TERMINAL_AGENT_HELPER" --terminal-agent-run {program} -- "$@"; }}
    fi
elif [ -n "${{BASH_VERSION-}}" ]; then
    if [ "$(type -t {program})" != function ] && [ "$(type -t {program})" != alias ]; then
        function {program} {{ "$ZED_TERMINAL_AGENT_HELPER" --terminal-agent-run {program} -- "$@"; }}
    fi
fi
"#));
                fish_script.push_str(&format!("if not functions -q {program}\nfunction {program}\n command $ZED_TERMINAL_AGENT_HELPER --terminal-agent-run {program} -- $argv\nend\nend\n"));
            }
            fs::write(directory.path().join("integration.sh"), script)?;
            fs::write(directory.path().join("integration.fish"), fish_script)?;
            Ok(Self {
                directory,
                listener: smol::Async::new(listener)?,
                state: SessionState::default(),
            })
        }

        pub fn environment(&self, shell: &str) -> Result<HashMap<String, String>> {
            Ok(HashMap::from([
                ("ZED_TERMINAL_AGENT_SHELL".into(), shell.into()),
                (
                    SOCKET.into(),
                    self.directory
                        .path()
                        .join("events")
                        .to_string_lossy()
                        .into_owned(),
                ),
                (
                    HELPER.into(),
                    std::env::current_exe()?.to_string_lossy().into_owned(),
                ),
                (
                    "ZED_TERMINAL_AGENT_SCRIPT".into(),
                    self.directory.path().to_string_lossy().into_owned(),
                ),
            ]))
        }

        pub async fn next_session(&mut self) -> Result<Option<TerminalAgentSession>> {
            loop {
                let (stream, _) = self.listener.accept().await?;
                let mut input = Vec::new();
                // One slow or malformed hook must not block subsequent lifecycle events.
                let mut stream = stream.take(MAX_EVENT_BYTES + 1);
                let read = stream.read_to_end(&mut input);
                let read = smol::future::race(async { read.await.map(|_| ()) }, async {
                    smol::Timer::after(std::time::Duration::from_secs(2)).await;
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "terminal agent event timeout",
                    ))
                })
                .await;
                if let Err(error) = read {
                    log::warn!("terminal agent event: {error}");
                    continue;
                }
                if input.len() as u64 > MAX_EVENT_BYTES {
                    continue;
                }
                match serde_json::from_slice(&input) {
                    Ok(event) => {
                        if self.state.apply(event) {
                            return Ok(self.state.session.clone());
                        }
                    }
                    Err(error) => log::warn!("invalid terminal agent event: {error}"),
                }
            }
        }
    }

    fn report(event: Event) -> Result<()> {
        let Some(socket) = std::env::var_os(SOCKET) else {
            return Ok(());
        };
        let mut stream = UnixStream::connect(socket)?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
        serde_json::to_writer(&mut stream, &event)?;
        Ok(())
    }

    pub fn run_helper_if_requested() -> Option<i32> {
        let mut arguments = std::env::args_os().skip(1);
        let mode = arguments.next()?;
        if mode == "--terminal-agent-hook" {
            if let Err(error) = report_hook(
                arguments
                    .next()
                    .and_then(|argument| argument.into_string().ok())
                    .as_deref(),
            ) {
                eprintln!("Zed terminal session hook: {error:#}");
            }
            return Some(0);
        }
        if mode != "--terminal-agent-run" {
            return None;
        }
        let result = run_agent(arguments.collect());
        Some(match result {
            Ok(code) => code,
            Err(error) => {
                eprintln!("Zed terminal agent: {error:#}");
                1
            }
        })
    }

    fn run_agent(mut arguments: Vec<std::ffi::OsString>) -> Result<i32> {
        if arguments.is_empty() {
            bail!("missing agent program");
        }
        let agent = match arguments.remove(0).to_str() {
            Some("claude") => TerminalAgent::Claude,
            Some("codex") => TerminalAgent::Codex,
            _ => bail!("unsupported terminal agent"),
        };
        if arguments.first().is_some_and(|argument| argument == "--") {
            arguments.remove(0);
        }
        let executable = which::which(agent.program())
            .with_context(|| format!("{} is not installed", agent.program()))?;
        if let Err(error) = install_hook(agent) {
            eprintln!("Zed could not install the session hook: {error:#}");
        }
        let launch = uuid::Uuid::new_v4().to_string();
        let session = TerminalAgentSession {
            agent,
            session_id: explicit_session_id(agent, &arguments),
            working_directory: std::env::current_dir()?,
        };
        if let Err(error) = report(Event::Started {
            launch: launch.clone(),
            session,
        }) {
            eprintln!("Zed session tracking: {error:#}");
        }
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env(AGENT, agent.program())
            .env(OWNER, std::process::id().to_string())
            .env(LAUNCH, &launch)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        // The CLI and its parent share the terminal foreground process group.
        // Only the CLI should handle Ctrl-C, while the parent records its exit.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
            command.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                Ok(())
            });
        }
        let status = command.status();
        if let Err(error) = report(Event::Exited { launch }) {
            eprintln!("Zed session tracking: {error:#}");
        }
        let status = status?;
        Ok(status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(1))
    }

    fn explicit_session_id(
        agent: TerminalAgent,
        arguments: &[std::ffi::OsString],
    ) -> Option<String> {
        let flag = if agent == TerminalAgent::Claude {
            "--resume"
        } else {
            "resume"
        };
        arguments.windows(2).find_map(|pair| {
            if pair[0] != flag {
                return None;
            }
            let session_id = pair[1].to_str()?;
            uuid::Uuid::parse_str(session_id).ok()?;
            Some(session_id.to_owned())
        })
    }

    fn report_hook(agent: Option<&str>) -> Result<()> {
        if agent != std::env::var(AGENT).ok().as_deref() {
            return Ok(());
        }
        let Some(launch) = std::env::var(LAUNCH).ok() else {
            return Ok(());
        };
        let Some(owner) = std::env::var(OWNER)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return Ok(());
        };
        if !owned_hook(owner) {
            return Ok(());
        }
        let mut input = Vec::new();
        std::io::stdin()
            .take(MAX_EVENT_BYTES + 1)
            .read_to_end(&mut input)?;
        if input.len() as u64 > MAX_EVENT_BYTES {
            bail!("hook input exceeds size limit");
        }
        let payload: serde_json::Value = serde_json::from_slice(&input)?;
        if payload
            .get("hook_event_name")
            .and_then(|value| value.as_str())
            != Some("SessionStart")
            || payload.get("agent_id").is_some()
        {
            return Ok(());
        }
        let Some(session_id) = payload.get("session_id").and_then(|value| value.as_str()) else {
            return Ok(());
        };
        if uuid::Uuid::parse_str(session_id).is_err() {
            return Ok(());
        }
        if agent == Some("codex")
            && let Ok(thread_id) = std::env::var("CODEX_THREAD_ID")
            && thread_id != session_id
        {
            return Ok(());
        }
        report(Event::Session {
            launch,
            session_id: session_id.to_owned(),
        })
    }

    fn owned_hook(owner: u32) -> bool {
        let mut system = System::new();
        let mut current = Pid::from_u32(std::process::id());
        for _ in 0..32 {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[current]),
                false,
                ProcessRefreshKind::nothing().with_cmd(UpdateKind::OnlyIfNotSet),
            );
            let Some(process) = system.process(current) else {
                return false;
            };
            // A shared backend can retain another terminal's environment. Its
            // SessionStart notifications cannot identify the initiating TUI.
            if process
                .cmd()
                .iter()
                .any(|argument| argument == "app-server")
            {
                return false;
            }
            if current.as_u32() == owner {
                return process
                    .cmd()
                    .iter()
                    .any(|argument| argument == "--terminal-agent-run");
            }
            let Some(parent) = process.parent() else {
                return false;
            };
            current = parent;
        }
        false
    }

    fn install_hook(agent: TerminalAgent) -> Result<()> {
        let home = dirs::home_dir().context("no home directory")?;
        let directory = match agent {
            TerminalAgent::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude")),
            TerminalAgent::Codex => std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
        };
        install_hook_in(&directory, agent)
    }

    fn install_hook_in(directory: &Path, agent: TerminalAgent) -> Result<()> {
        fs::create_dir_all(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(".zed-session-hook.lock"))?;
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&lock), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let path = directory.join(if agent == TerminalAgent::Claude {
            "settings.json"
        } else {
            "hooks.json"
        });
        let path = if fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            fs::canonicalize(path)?
        } else {
            path
        };
        let original = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}".into(),
            Err(error) => return Err(error.into()),
        };
        let mut settings: serde_json::Value = serde_json_lenient::from_str(&original)?;
        let object = settings
            .as_object_mut()
            .context("agent settings must be an object")?;
        let hooks = object
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("hooks must be an object")?;
        let hook_command = HOOK.replace("{agent}", agent.program());
        let groups = hooks
            .entry("SessionStart")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .context("SessionStart hooks must be an array")?;
        if groups.iter().any(|group| {
            group
                .get("hooks")
                .and_then(|hooks| hooks.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("command").and_then(|command| command.as_str())
                            == Some(hook_command.as_str())
                    })
                })
        }) {
            return Ok(());
        }
        groups.push(
            serde_json::json!({ "hooks": [{ "type": "command", "command": hook_command, "timeout": 5 }] }),
        );
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        if let Ok(metadata) = fs::metadata(&path) {
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
        }
        serde_json::to_writer_pretty(&mut temporary, &settings)?;
        temporary.write_all(b"\n")?;
        temporary.persist(&path)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn hook_installation_preserves_settings_and_is_idempotent() {
            for agent in [TerminalAgent::Claude, TerminalAgent::Codex] {
                let directory = tempfile::tempdir().unwrap();
                let filename = if agent == TerminalAgent::Claude {
                    "settings.json"
                } else {
                    "hooks.json"
                };
                let path = directory.path().join(filename);
                fs::write(&path, r#"{"permissions":{"allow":["Read"]},"hooks":{"SessionStart":[{"matcher":"resume","hooks":[{"type":"command","command":"existing-hook"}]}]}}"#).unwrap();
                install_hook_in(directory.path(), agent).unwrap();
                let once = fs::read_to_string(&path).unwrap();
                install_hook_in(directory.path(), agent).unwrap();
                assert_eq!(once, fs::read_to_string(&path).unwrap());
                let settings: serde_json::Value = serde_json::from_str(&once).unwrap();
                assert_eq!(settings["permissions"]["allow"][0], "Read");
                let command = settings["hooks"]["SessionStart"][1]["hooks"][0]["command"]
                    .as_str()
                    .unwrap();
                assert!(command.contains("$ZED_TERMINAL_AGENT_SOCKET"));
                assert!(command.contains(&format!("--terminal-agent-hook {};", agent.program())));
                assert_eq!(
                    settings["hooks"]["SessionStart"].as_array().unwrap().len(),
                    2
                );
                assert_eq!(
                    settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
                    "existing-hook"
                );
            }
        }
        #[test]
        fn stale_hooks_and_exits_cannot_change_another_launch() {
            let mut state = SessionState::default();
            let session = TerminalAgentSession {
                agent: TerminalAgent::Codex,
                session_id: None,
                working_directory: "/project".into(),
            };
            assert!(state.apply(Event::Started {
                launch: "new".into(),
                session: session.clone()
            }));
            assert!(!state.apply(Event::Session {
                launch: "old".into(),
                session_id: uuid::Uuid::new_v4().to_string()
            }));
            assert!(!state.apply(Event::Exited {
                launch: "old".into()
            }));
            assert_eq!(state.session, Some(session));
            assert!(state.apply(Event::Exited {
                launch: "new".into()
            }));
            assert!(state.session.is_none());
        }
        #[test]
        fn hook_installation_preserves_configuration_symlinks() {
            let directory = tempfile::tempdir().unwrap();
            let target = directory.path().join("linked-settings.json");
            fs::write(&target, "{}\n").unwrap();
            let link = directory.path().join("settings.json");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            install_hook_in(directory.path(), TerminalAgent::Claude).unwrap();
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            let settings: serde_json::Value =
                serde_json::from_slice(&fs::read(target).unwrap()).unwrap();
            assert_eq!(
                settings["hooks"]["SessionStart"].as_array().unwrap().len(),
                1
            );
        }

        #[test]
        fn terminal_sockets_keep_concurrent_conversations_separate() {
            let mut first = TerminalAgentIntegration::new().unwrap();
            let mut second = TerminalAgentIntegration::new().unwrap();
            let first_session = TerminalAgentSession {
                agent: TerminalAgent::Claude,
                session_id: Some(uuid::Uuid::new_v4().to_string()),
                working_directory: "/first".into(),
            };
            let second_session = TerminalAgentSession {
                agent: TerminalAgent::Codex,
                session_id: None,
                working_directory: "/second".into(),
            };
            for (integration, session) in [(&first, &first_session), (&second, &second_session)] {
                let mut stream =
                    UnixStream::connect(integration.directory.path().join("events")).unwrap();
                serde_json::to_writer(
                    &mut stream,
                    &Event::Started {
                        launch: uuid::Uuid::new_v4().to_string(),
                        session: session.clone(),
                    },
                )
                .unwrap();
            }
            assert_eq!(
                smol::block_on(first.next_session()).unwrap(),
                Some(first_session)
            );
            assert_eq!(
                smol::block_on(second.next_session()).unwrap(),
                Some(second_session)
            );
        }

        #[test]
        fn invalid_settings_are_left_untouched() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("settings.json");
            fs::write(&path, "invalid settings").unwrap();
            assert!(install_hook_in(directory.path(), TerminalAgent::Claude).is_err());
            assert_eq!(fs::read_to_string(path).unwrap(), "invalid settings");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_uses_only_the_recorded_conversation() {
        let session_id = uuid::Uuid::new_v4().to_string();
        for (agent, flag) in [
            (TerminalAgent::Claude, "--resume"),
            (TerminalAgent::Codex, "resume"),
        ] {
            let mut session = TerminalAgentSession {
                agent,
                session_id: Some(session_id.clone()),
                working_directory: "/project".into(),
            };
            assert_eq!(
                session.resume_command(),
                format!("{} {flag} {session_id}", agent.program())
            );
            session.session_id = None;
            assert_eq!(session.resume_command(), agent.program());
        }
    }
}
