use std::{collections::HashSet, path::Path, process::Command};

use anyhow::{Context as _, Result, anyhow, bail};
use collections::HashMap;
use gpui::{AppContext as _, Entity, Task};
use project::Project;
use regex::Regex;
use remote::Interactive;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ui::{App, IconName};
use util::paths::PathStyle;

const MAX_HERDR_SESSION_NAME_BYTES: usize = 64;
pub const INSTALL_COMMAND: &str = "curl -fsSL https://herdr.dev/install.sh | sh";
const MISSING_HERDR: &str = "zed: herdr executable not found";
const LAUNCH_SCRIPT: &str = r#"if command -v herdr >/dev/null 2>&1; then
    exec herdr "$@"
elif [ -x "${HERDR_INSTALL_DIR:-$HOME/.local/bin}/herdr" ]; then
    exec "${HERDR_INSTALL_DIR:-$HOME/.local/bin}/herdr" "$@"
else
    echo "zed: herdr executable not found" >&2
    exit 127
fi"#;

#[derive(Debug)]
pub struct HerdrNotInstalled;

impl std::fmt::Display for HerdrNotInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Herdr is not installed on the terminal host")
    }
}

impl std::error::Error for HerdrNotInstalled {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrTerminalSession {
    pub name: String,
    pub agent: Option<String>,
    pub agent_session: Option<HerdrAgentSession>,
    #[serde(default)]
    pub current_title: Option<String>,
    #[serde(default)]
    pub is_working: bool,
    #[serde(default)]
    pub initial_command: Option<String>,
    #[serde(default)]
    pub initial_command_sent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrAgentSession {
    pub source: String,
    pub agent: String,
    pub kind: String,
    pub value: String,
}

impl HerdrTerminalSession {
    fn snapshot_response(snapshot: &Value) -> &Value {
        let response = snapshot.get("result").unwrap_or(snapshot);
        response.get("snapshot").unwrap_or(response)
    }

    pub fn icon(&self) -> IconName {
        IconName::Herdr
    }

    pub fn display_title(&self) -> &str {
        self.current_title.as_deref().unwrap_or(&self.name)
    }

    pub fn attach_command(&self, path_style: PathStyle) -> String {
        // A Herdr name is restricted to ASCII letters, digits, dots, underscores,
        // and hyphens, so this command cannot acquire shell syntax from a path.
        if path_style.is_posix() {
            format!("sh -c '{LAUNCH_SCRIPT}' zed-herdr --session {}", self.name)
        } else {
            format!("herdr --session {}", self.name)
        }
    }

    pub fn update_from_snapshot(&mut self, snapshot: &Value) -> bool {
        let response = Self::snapshot_response(snapshot);
        let Some(panes) = response.get("panes").and_then(Value::as_array) else {
            return false;
        };
        let focused_pane_id = response.get("focused_pane_id").and_then(Value::as_str);
        let pane = focused_pane_id
            .and_then(|id| {
                panes
                    .iter()
                    .find(|pane| pane.get("pane_id").and_then(Value::as_str) == Some(id))
            })
            .or_else(|| panes.first());
        let Some(pane) = pane else {
            return false;
        };
        let is_working = panes
            .iter()
            .any(|pane| pane.get("agent_status").and_then(Value::as_str) == Some("working"));
        let agent_session = pane
            .get("agent_session")
            .and_then(|session| serde_json::from_value(session.clone()).ok());
        let agent = pane
            .get("agent")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                agent_session
                    .as_ref()
                    .map(|session: &HerdrAgentSession| session.agent.clone())
            });
        let current_title = agent.as_ref().and_then(|_| {
            let title = pane
                .get("terminal_title_stripped")
                .and_then(Value::as_str)
                .or_else(|| pane.get("terminal_title").and_then(Value::as_str))?
                .trim();
            let workspace_label =
                pane.get("workspace_id")
                    .and_then(Value::as_str)
                    .and_then(|workspace_id| {
                        response
                            .get("workspaces")
                            .and_then(Value::as_array)?
                            .iter()
                            .find(|workspace| {
                                workspace.get("workspace_id").and_then(Value::as_str)
                                    == Some(workspace_id)
                            })
                            .and_then(|workspace| workspace.get("label"))
                            .and_then(Value::as_str)
                    });
            let title = workspace_label
                .and_then(|label| title.strip_suffix(&format!(" | {label}")))
                .unwrap_or(title)
                .trim();
            (!title.is_empty() && title != self.name && Some(title) != workspace_label)
                .then(|| title.to_owned())
        });
        if self.agent == agent
            && self.agent_session == agent_session
            && self.current_title == current_title
            && self.is_working == is_working
        {
            return false;
        }
        self.agent = agent;
        self.agent_session = agent_session;
        self.current_title = current_title;
        self.is_working = is_working;
        true
    }

    pub fn focused_pane_id(snapshot: &Value) -> Option<&str> {
        let response = Self::snapshot_response(snapshot);
        response.get("focused_pane_id").and_then(Value::as_str)
    }
}

pub fn session_base_name(path: &Path, pattern: Option<&str>) -> Result<String> {
    let matched = if let Some(pattern) = pattern {
        let regex = Regex::new(pattern).context("invalid Herdr session name regex")?;
        regex
            .captures(&path.to_string_lossy())
            .and_then(|captures| {
                captures
                    .get(1)
                    .or_else(|| captures.get(0))
                    .map(|capture| capture.as_str().to_owned())
            })
    } else {
        None
    };
    let source = matched.filter(|value| !value.is_empty()).or_else(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    let source = source.ok_or_else(|| anyhow!("workspace has no directory name"))?;
    let name: String = source
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches(|character| character == '-' || character == '.');
    if name.is_empty() {
        bail!("workspace path yielded an empty Herdr session name");
    }
    Ok(name
        .get(..name.len().min(MAX_HERDR_SESSION_NAME_BYTES))
        .unwrap_or(name)
        .trim_end_matches(|character| character == '-' || character == '.')
        .to_owned())
}

pub fn available_name(base: &str, occupied: &HashSet<String>) -> String {
    if !occupied.contains(base) {
        return base.to_owned();
    }
    let mut suffix = 1u64;
    loop {
        let suffix_text = format!("-{suffix}");
        let prefix_length = MAX_HERDR_SESSION_NAME_BYTES.saturating_sub(suffix_text.len());
        let candidate = format!(
            "{}{}",
            base.get(..base.len().min(prefix_length))
                .unwrap_or(base)
                .trim_end_matches(|character| character == '-' || character == '.'),
            suffix_text
        );
        if !occupied.contains(&candidate) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

pub fn command(
    project: Entity<Project>,
    args: &[String],
    session: Option<&str>,
    cx: &mut App,
) -> Task<Result<Command>> {
    let mut environment = HashMap::default();
    if let Some(session) = session {
        environment.insert("HERDR_SESSION".to_owned(), session.to_owned());
    }
    let (program, args) = if project.read(cx).path_style(cx).is_posix() {
        let mut launch_args = vec![
            "-c".to_owned(),
            LAUNCH_SCRIPT.to_owned(),
            "zed-herdr".to_owned(),
        ];
        launch_args.extend_from_slice(args);
        ("sh", launch_args)
    } else {
        ("herdr", args.to_vec())
    };
    host_command(project, program, &args, environment, cx)
}

pub fn install_command(project: Entity<Project>, cx: &mut App) -> Task<Result<Command>> {
    host_command(
        project,
        "bash",
        &[
            "-o".to_owned(),
            "pipefail".to_owned(),
            "-c".to_owned(),
            INSTALL_COMMAND.to_owned(),
        ],
        HashMap::default(),
        cx,
    )
}

pub fn list_sessions(project: Entity<Project>, cx: &mut App) -> Task<Result<HashSet<String>>> {
    let command = command(
        project,
        &["session".to_owned(), "list".to_owned(), "--json".to_owned()],
        None,
        cx,
    );
    cx.spawn(async move |cx| {
        let command = command.await?;
        cx.background_spawn(async move { session_names(&run(command)?) })
            .await
    })
}

fn host_command(
    project: Entity<Project>,
    program: &str,
    args: &[String],
    environment: HashMap<String, String>,
    cx: &mut App,
) -> Task<Result<Command>> {
    let (remote_client, directory, project_environment) = {
        let project = project.read(cx);
        (
            project.remote_client(),
            project.active_project_directory(cx),
            project.environment().clone(),
        )
    };
    if let Some(remote_client) = remote_client {
        let Some(directory) = directory else {
            return Task::ready(Err(anyhow!("No remote project directory for Herdr")));
        };
        // SSH's noninteractive PATH can omit user-installed binaries. Use the
        // same cached login-shell environment as the project's terminals.
        let shell_environment = project_environment.update(cx, |environment, cx| {
            environment.directory_environment(directory, cx)
        });
        let args = args.to_vec();
        let program = program.to_owned();
        cx.spawn(async move |cx| {
            let mut shell_environment = shell_environment
                .await
                .context("Could not load the remote shell environment for Herdr")?;
            shell_environment.extend(environment);
            let template = remote_client.read_with(cx, |client, _| {
                client.build_command(
                    Some(program),
                    &args,
                    &shell_environment,
                    None,
                    None,
                    Interactive::No,
                )
            })?;
            let mut command = Command::new(template.program);
            command.args(template.args).envs(template.env);
            Ok(command)
        })
    } else {
        let mut command = Command::new(program);
        command.args(args).envs(environment);
        Task::ready(Ok(command))
    }
}

pub fn run(mut command: Command) -> Result<Value> {
    let output = command.output().context("run Herdr command")?;
    if output.status.code() == Some(127)
        && String::from_utf8_lossy(&output.stderr).trim() == MISSING_HERDR
    {
        return Err(HerdrNotInstalled.into());
    }
    let response: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    if let Some(error) = response.as_ref().and_then(|value| value.get("error")) {
        bail!("Herdr returned an error: {error}");
    }
    if !output.status.success() {
        bail!(
            "Herdr command failed: {}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    if output.stdout.is_empty() {
        return Ok(Value::Null);
    }
    response.ok_or_else(|| anyhow!("Herdr returned an invalid JSON response"))
}

pub fn install(mut command: Command) -> Result<()> {
    let output = command.output().context("run Herdr installer")?;
    if !output.status.success() {
        bail!(
            "Herdr installation failed: {}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    Ok(())
}

pub fn session_names(value: &Value) -> Result<HashSet<String>> {
    let sessions = value
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Herdr returned no session list"))?;
    Ok(sessions
        .iter()
        .filter_map(|session| session.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn missing_binary_is_distinct_from_herdr_and_transport_errors() {
        let directory = tempfile::tempdir().unwrap();
        let mut missing = Command::new("/bin/sh");
        missing
            .args([
                "-c",
                LAUNCH_SCRIPT,
                "zed-herdr",
                "session",
                "list",
                "--json",
            ])
            .env("HOME", directory.path())
            .env("PATH", directory.path())
            .env_remove("HERDR_INSTALL_DIR");
        assert!(run(missing).unwrap_err().is::<HerdrNotInstalled>());

        for script in [
            "echo 'Herdr failed' >&2; exit 127",
            "echo 'SSH failed' >&2; exit 255",
            "echo '{\"error\":\"server unavailable\"}'",
        ] {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            assert!(!run(command).unwrap_err().is::<HerdrNotInstalled>());
        }
    }

    #[cfg(unix)]
    #[test]
    fn installed_binary_outside_path_runs_with_arguments_and_session() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let binary = bin.join("herdr");
        std::fs::write(&binary, "#!/bin/sh\nprintf '{\"session\":\"%s\",\"argument\":\"%s\"}' \"$HERDR_SESSION\" \"$1\"\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", LAUNCH_SCRIPT, "zed-herdr", "argument with spaces"])
            .env("HOME", directory.path())
            .env("PATH", directory.path())
            .env("HERDR_SESSION", "workspace-1")
            .env_remove("HERDR_INSTALL_DIR");
        assert_eq!(
            run(command).unwrap(),
            serde_json::json!({"session":"workspace-1", "argument":"argument with spaces"})
        );
    }

    #[test]
    fn names_from_workspace_path_and_collisions() {
        let path = Path::new(
            "/home/vatolin/arcadia-worktrees/sync-perfomance-CLEANWEB-6797/yweb/antispam/clean_web/pingers",
        );
        let base = session_base_name(path, Some("/arcadia-worktrees/([^/]+)"))
            .expect("valid session name");
        assert_eq!(base, "sync-perfomance-CLEANWEB-6797");
        let occupied = HashSet::from([base.clone(), format!("{base}-1")]);
        assert_eq!(available_name(&base, &occupied), format!("{base}-2"));
        let long_base = session_base_name(Path::new(&format!("/{}", "a".repeat(70))), None)
            .expect("long path still yields a valid name");
        assert_eq!(long_base.len(), 64);
        let candidate = available_name(&long_base, &HashSet::from([long_base.clone()]));
        assert_eq!(candidate.len(), 64);
        assert!(candidate.ends_with("-1"));
    }

    #[test]
    fn pane_snapshot_tracks_session_switches() {
        let mut session = HerdrTerminalSession {
            name: "project".to_owned(),
            agent: None,
            agent_session: None,
            current_title: None,
            is_working: false,
            initial_command: None,
            initial_command_sent: false,
        };
        let codex = serde_json::json!({"result": {"snapshot": {"focused_pane_id": "w1:p1", "panes": [{"pane_id": "w1:p1", "agent": "codex", "agent_session": {"source": "herdr:codex", "agent": "codex", "kind": "id", "value": "first"}}]}}});
        assert!(session.update_from_snapshot(&codex));
        assert_eq!(
            session
                .agent_session
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("first")
        );
        let next_codex = serde_json::json!({"result": {"snapshot": {"focused_pane_id": "w1:p1", "panes": [{"pane_id": "w1:p1", "agent": "codex", "agent_session": {"source": "herdr:codex", "agent": "codex", "kind": "id", "value": "next"}}]}}});
        assert!(session.update_from_snapshot(&next_codex));
        assert_eq!(
            session
                .agent_session
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("next")
        );
        let claude = serde_json::json!({"result": {"snapshot": {"focused_pane_id": "w1:p1", "panes": [{"pane_id": "w1:p1", "agent": "claude", "agent_session": {"source": "herdr:claude", "agent": "claude", "kind": "id", "value": "second"}}]}}});
        assert!(session.update_from_snapshot(&claude));
        assert_eq!(session.icon(), IconName::Herdr);
        assert_eq!(
            session
                .agent_session
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("second")
        );
    }

    #[test]
    fn pane_status_tracks_work_across_the_session() {
        let mut session = HerdrTerminalSession {
            name: "project".to_owned(),
            agent: None,
            agent_session: None,
            current_title: None,
            is_working: false,
            initial_command: None,
            initial_command_sent: false,
        };
        let snapshot = |status: &str| {
            serde_json::json!({"result": {"snapshot": {
                "focused_pane_id": "w1:p1",
                "panes": [
                    {"pane_id": "w1:p1", "agent_status": "idle"},
                    {"pane_id": "w1:p2", "agent": "codex", "agent_status": status}
                ]
            }}})
        };

        assert!(session.update_from_snapshot(&snapshot("working")));
        assert!(session.is_working);
        assert!(!session.update_from_snapshot(&snapshot("working")));
        assert!(session.update_from_snapshot(&snapshot("idle")));
        assert!(!session.is_working);
    }

    #[test]
    fn pane_title_tracks_current_agent_conversation() {
        let mut session = HerdrTerminalSession {
            name: "food-photo-apps-2".to_owned(),
            agent: None,
            agent_session: None,
            current_title: None,
            is_working: false,
            initial_command: None,
            initial_command_sent: false,
        };
        let snapshot = |title: &str| {
            serde_json::json!({
                "result": {"snapshot": {
                    "focused_pane_id": "w1:p1",
                    "workspaces": [{"workspace_id": "w1", "label": "food-photo-apps"}],
                    "panes": [{
                        "pane_id": "w1:p1",
                        "workspace_id": "w1",
                        "agent": "codex",
                        "terminal_title_stripped": title
                    }]
                }}
            })
        };

        assert!(session.update_from_snapshot(&snapshot("Respond to greeting | food-photo-apps")));
        assert_eq!(session.display_title(), "Respond to greeting");
        assert!(!session.update_from_snapshot(&snapshot("Respond to greeting | food-photo-apps")));
        assert!(
            session.update_from_snapshot(&snapshot("Investigate failing tests | food-photo-apps"))
        );
        assert_eq!(session.display_title(), "Investigate failing tests");
        assert!(session.update_from_snapshot(&snapshot("food-photo-apps")));
        assert_eq!(session.display_title(), "food-photo-apps-2");
    }
}
