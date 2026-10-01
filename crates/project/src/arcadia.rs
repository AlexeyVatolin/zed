use crate::{Project, ProjectPath};
use anyhow::{Context as _, Result, anyhow, ensure};
use collections::HashMap;
use gpui::{AppContext as _, Context, Task};
use rpc::proto;
use serde::Deserialize;
use std::{path::Path, time::Duration};
use url::Url;

impl Project {
    /// Runs only after the user invokes a copy action. SSH returns the URL;
    /// the desktop editor owns the clipboard.
    pub fn get_arcadia_link(
        &self,
        path: ProjectPath,
        start_row: u32,
        end_row: u32,
        current_branch: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        if let Some(remote) = &self.remote_client {
            let client = remote.read(cx).proto_client();
            return cx.background_spawn(async move {
                Ok(client
                    .request(proto::GetArcadiaLink {
                        project_id: proto::REMOTE_SERVER_PROJECT_ID,
                        path: Some(path.to_proto()),
                        start_row,
                        end_row,
                        current_branch,
                    })
                    .await?
                    .url)
            });
        }
        if !self.is_local() {
            return Task::ready(Err(anyhow!("Arcadia links require a local or SSH project")));
        }
        let Some(worktree) = self.worktree_for_id(path.worktree_id, cx) else {
            return Task::ready(Err(anyhow!("file worktree is no longer open")));
        };
        let file = worktree.read(cx).absolutize(&path.path);
        let environment = self.environment.update(cx, |environment, cx| {
            environment.worktree_environment(worktree, cx)
        });
        cx.background_spawn(async move {
            resolve_arcadia_link(&file, start_row, end_row, current_branch, environment.await).await
        })
    }
}

/// Shared by the desktop and SSH server. No network fetches or repository writes.
pub async fn resolve_arcadia_link(
    file: &Path,
    start_row: u32,
    end_row: u32,
    current_branch: bool,
    environment: Option<HashMap<String, String>>,
) -> Result<String> {
    let file = smol::fs::canonicalize(file)
        .await
        .context("file is not available on disk")?;
    let directory = file.parent().context("file has no parent directory")?;
    let environment = environment.unwrap_or_default();
    let path = environment
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    let arc =
        which::which_in("arc", path, directory).context("arc was not found in the project PATH")?;
    resolve_with_arc(
        &arc,
        &file,
        start_row,
        end_row,
        current_branch,
        &environment,
    )
    .await
}

async fn resolve_with_arc(
    arc: &Path,
    file: &Path,
    start_row: u32,
    end_row: u32,
    current_branch: bool,
    environment: &HashMap<String, String>,
) -> Result<String> {
    let directory = file.parent().context("file has no parent directory")?;
    let (root, revision) = if current_branch {
        let (root, info) = futures::try_join!(
            run_arc(arc, directory, &["root"], environment),
            run_arc(arc, directory, &["info", "--json"], environment),
        )?;
        #[derive(Deserialize)]
        struct ArcInfo {
            remote: Option<String>,
        }
        let info: ArcInfo = serde_json::from_str(&info).context("invalid arc info response")?;
        let remote = info
            .remote
            .filter(|remote| !remote.is_empty())
            .context("current Arc branch has no upstream; push it before copying a branch link")?;
        (root, remote)
    } else {
        (
            run_arc(arc, directory, &["root"], environment).await?,
            "trunk".to_owned(),
        )
    };
    build_arcadia_link(
        file,
        Path::new(root.trim_end_matches(['\r', '\n'])),
        &revision,
        start_row,
        end_row,
    )
}

async fn run_arc(
    arc: &Path,
    directory: &Path,
    args: &[&str],
    environment: &HashMap<String, String>,
) -> Result<String> {
    let mut command = util::command::new_command(arc);
    command
        .args(args)
        .current_dir(directory)
        .envs(environment)
        .env("ARC_DISABLE_TELEMETRY", "yes")
        .kill_on_drop(true);
    let output = smol::future::or(
        async { command.output().await.context("failed to run arc") },
        async {
            smol::Timer::after(Duration::from_secs(5)).await;
            Err(anyhow!("arc {} timed out", args.join(" ")))
        },
    )
    .await?;
    ensure!(
        output.status.success(),
        "arc {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("arc output is not UTF-8")
}

fn build_arcadia_link(
    file: &Path,
    root: &Path,
    revision: &str,
    start_row: u32,
    end_row: u32,
) -> Result<String> {
    ensure!(root.is_absolute(), "arc returned an invalid root");
    ensure!(start_row <= end_row, "invalid line range");
    let relative = file
        .strip_prefix(root)
        .context("file is outside the Arc root")?;
    ensure!(!relative.as_os_str().is_empty(), "link requires a file");
    let mut url = Url::parse("https://a.yandex-team.ru/arcadia/")?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow!("invalid Arcadia URL"))?;
        segments.pop_if_empty();
        for component in relative.components() {
            ensure!(
                matches!(component, std::path::Component::Normal(_)),
                "invalid file path"
            );
            segments.push(
                component
                    .as_os_str()
                    .to_str()
                    .context("file path is not UTF-8")?,
            );
        }
    }
    url.query_pairs_mut().append_pair("rev", revision);
    let start = u64::from(start_row) + 1;
    let end = u64::from(end_row) + 1;
    url.set_fragment(Some(&if start == end {
        format!("L{start}")
    } else {
        format!("L{start}-{end}")
    }));
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_encode_paths_and_branches_and_use_one_based_lines() {
        let root = Path::new("/arcadia");
        let file = root.join("dir/a #é.txt");
        let url = build_arcadia_link(&file, root, "users/user/branch & fix", 41, 56).unwrap();
        let url = Url::parse(&url).unwrap();
        assert_eq!(url.path(), "/arcadia/dir/a%20%23%C3%A9.txt");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("rev".into(), "users/user/branch & fix".into())]
        );
        assert_eq!(url.fragment(), Some("L42-57"));
        assert_eq!(
            build_arcadia_link(&root.join("file.rs"), root, "trunk", 0, 0).unwrap(),
            "https://a.yandex-team.ru/arcadia/file.rs?rev=trunk#L1"
        );
    }

    #[test]
    fn rejects_files_outside_root_and_invalid_ranges() {
        assert!(
            build_arcadia_link(
                Path::new("/arcadia-other/file"),
                Path::new("/arcadia"),
                "trunk",
                0,
                0
            )
            .is_err()
        );
        assert!(
            build_arcadia_link(
                Path::new("/arcadia/file"),
                Path::new("/arcadia"),
                "trunk",
                2,
                1
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn arcadia_resolver_reads_upstream_without_fetching_or_pushing() {
        use std::os::unix::fs::PermissionsExt as _;
        smol::block_on(async {
            let temporary = tempfile::tempdir().unwrap();
            let directory = temporary.path().canonicalize().unwrap();
            let arc = directory.join("arc");
            std::fs::write(&arc, "#!/bin/sh\ncase \"$*\" in\nroot) pwd ;;\n'info --json') printf '%s' '{\"remote\":\"users/test/feature\"}' ;;\n*) exit 99 ;;\nesac\n").unwrap();
            std::fs::set_permissions(&arc, std::fs::Permissions::from_mode(0o755)).unwrap();
            let file = directory.join("a #.rs");
            std::fs::write(&file, "test").unwrap();
            let environment = HashMap::default();
            let trunk = resolve_with_arc(&arc, &file, 3, 6, false, &environment)
                .await
                .unwrap();
            let branch = resolve_with_arc(&arc, &file, 3, 6, true, &environment)
                .await
                .unwrap();
            assert!(trunk.ends_with("?rev=trunk#L4-7"));
            assert!(branch.ends_with("?rev=users%2Ftest%2Ffeature#L4-7"));
            assert!(branch.contains("/arcadia/a%20%23.rs?"));
            std::fs::write(&arc, "#!/bin/sh\ncase \"$*\" in\nroot) pwd ;;\n'info --json') printf '%s' '{}' ;;\n*) exit 99 ;;\nesac\n").unwrap();
            let error = resolve_with_arc(&arc, &file, 0, 0, true, &environment)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("no upstream"));
            std::fs::write(
                &arc,
                "#!/bin/sh\necho 'not a mounted arc repository' >&2\nexit 1\n",
            )
            .unwrap();
            let error = resolve_with_arc(&arc, &file, 0, 0, false, &environment)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("not a mounted arc repository"));
        });
    }
}
