use std::path::PathBuf;

use anyhow::{Context as _, Result};
use editor::Editor;
use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Render,
    Task, WeakEntity, Window,
};
use remote::{RemoteClient, RemoteConnectionOptions};
use settings::{ArcWorkspacesConfig, SshConnection};
use ui::{ContextMenu, DropdownMenu, DropdownStyle, ModalHeader, prelude::*};
use util::ResultExt;
use workspace::{
    DismissDecision, ModalView, MultiWorkspace, OpenOptions, Workspace, arc_workspaces,
};

use crate::open_remote_project;

pub fn open_arc_workspace_modal(
    source: &Entity<Workspace>,
    options: RemoteConnectionOptions,
    create_new_window: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let source = source.clone();
    let window = window.window_handle();
    cx.defer(move |cx| {
        window
            .update(cx, |_, window, cx| {
                open_connected_arc_workspace_modal(&source, options, create_new_window, window, cx);
            })
            .log_err();
    });
}

fn open_connected_arc_workspace_modal(
    source: &Entity<Workspace>,
    options: RemoteConnectionOptions,
    create_new_window: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(server) = arc_workspaces::ssh_connection(&options, cx) else {
        return;
    };
    let Some(config) = server
        .arc_workspaces
        .clone()
        .filter(|config| !config.types.is_empty())
    else {
        return;
    };
    let task = remote_connection::connect_with_modal(source, options.clone(), window, cx);
    let source = source.clone();
    window
        .spawn(cx, async move |cx| {
            let result: Result<()> = async {
                let session = task.await?;
                remote_connection::dismiss_connection_modal(&source, cx);
                let Some(session) = session else {
                    return Ok(());
                };
                source.update_in(cx, |source_workspace, window, cx| {
                    let weak = cx.entity().downgrade();
                    source_workspace.toggle_modal(window, cx, |window, cx| {
                        ArcWorkspaceModal::new(
                            weak,
                            options,
                            server,
                            config,
                            session,
                            create_new_window,
                            window,
                            cx,
                        )
                    });
                })?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                remote_connection::dismiss_connection_modal(&source, cx);
                source.update(cx, |workspace, cx| workspace.show_error(error, cx));
            }
        })
        .detach();
}

pub fn delete_arc_workspace(
    source: &Entity<Workspace>,
    options: RemoteConnectionOptions,
    workspace: settings::ArcWorkspace,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<bool>> {
    let guard = match arc_workspaces::begin_deletion(&options, &workspace, cx) {
        Ok(guard) => guard,
        Err(error) => return Task::ready(Err(error)),
    };
    let prompt = arc_workspaces::confirm_deletion(&options, &workspace, window, cx);
    let source = source.clone();
    let window_handle = window.window_handle();
    let fs = source.read(cx).app_state().fs.clone();
    cx.spawn(async move |cx| {
        let _guard = guard;
        if prompt.await? != 1 {
            return Ok(false);
        }
        let connect = window_handle.update(cx, |_, window, cx| {
            remote_connection::connect_with_modal(&source, options.clone(), window, cx)
        })?;
        let result: Result<bool> = async {
            let Some(session) = connect.await? else {
                return Ok(false);
            };
            let connection = session
                .read_with(cx, |session, _| session.remote_connection())
                .context("Remote connection is unavailable")?;
            arc_workspaces::delete_workspace(&connection, &workspace).await?;
            arc_workspaces::finish_deletion(options, workspace, fs, cx).await?;
            Ok(true)
        }
        .await;
        window_handle
            .update(cx, |_, _, cx| {
                source.update(cx, |workspace, cx| {
                    if let Some(modal) =
                        workspace.active_modal::<remote_connection::RemoteConnectionModal>(cx)
                    {
                        modal.update(cx, |modal, cx| modal.finished(cx));
                    }
                });
            })
            .log_err();
        result
    })
}

struct ArcWorkspaceModal {
    source: WeakEntity<Workspace>,
    options: RemoteConnectionOptions,
    server: SshConnection,
    config: ArcWorkspacesConfig,
    session: Entity<RemoteClient>,
    name_editor: Entity<Editor>,
    workspace_type: String,
    create_new_window: bool,
    creating: bool,
    error: Option<String>,
}

impl ArcWorkspaceModal {
    fn new(
        source: WeakEntity<Workspace>,
        options: RemoteConnectionOptions,
        server: SshConnection,
        config: ArcWorkspacesConfig,
        session: Entity<RemoteClient>,
        create_new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_editor = cx.new(|cx| Editor::single_line(window, cx));
        name_editor.update(cx, |editor, cx| {
            editor.set_placeholder_text("Workspace name", window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        let workspace_type = config.types.keys().next().cloned().unwrap_or_default();
        Self {
            source,
            options,
            server,
            config,
            session,
            name_editor,
            workspace_type,
            create_new_window,
            creating: false,
            error: None,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.creating {
            cx.emit(DismissEvent);
        }
    }

    fn submit(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let name = self.name_editor.read(cx).text(cx).trim().to_owned();
        if let Err(error) = arc_workspaces::validate_name(&name) {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let Some(connection) = self.session.read(cx).remote_connection() else {
            self.error = Some("Remote connection is unavailable".into());
            cx.notify();
            return;
        };
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let config = self.config.clone();
        let server = self.server.clone();
        let options = self.options.clone();
        let workspace_type = self.workspace_type.clone();
        let app_state = source.read(cx).app_state().clone();
        let window_handle = window.window_handle().downcast::<MultiWorkspace>();
        let requesting_window = if self.create_new_window {
            None
        } else {
            window_handle
        };
        self.creating = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: Result<()> = async {
                let workspace =
                    arc_workspaces::create_workspace(&connection, &config, name, workspace_type)
                        .await?;
                let saved = cx.update(|cx| {
                    arc_workspaces::save_workspace(
                        server,
                        workspace.clone(),
                        app_state.fs.clone(),
                        cx,
                    )
                });
                saved.await??;
                this.update(cx, |modal, cx| {
                    modal.creating = false;
                    cx.notify();
                })?;
                if let Some(window) = window_handle {
                    window.update(cx, |_, window, cx| {
                        source.update(cx, |source, cx| source.hide_modal(window, cx))
                    })?;
                }
                open_remote_project(
                    options,
                    vec![PathBuf::from(workspace.project_path)],
                    app_state,
                    OpenOptions {
                        requesting_window,
                        ..Default::default()
                    },
                    cx,
                )
                .await?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                this.update(cx, |modal, cx| {
                    modal.creating = false;
                    modal.error = Some(format!("{error:#}"));
                    cx.notify();
                })
                .log_err();
                source.update(cx, |source, cx| source.show_error(error, cx));
            }
        })
        .detach();
    }
}

impl ModalView for ArcWorkspaceModal {
    fn on_before_dismiss(&mut self, _: &mut Window, _: &mut Context<Self>) -> DismissDecision {
        DismissDecision::Dismiss(!self.creating)
    }
}
impl EventEmitter<DismissEvent> for ArcWorkspaceModal {}
impl Focusable for ArcWorkspaceModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name_editor.focus_handle(cx)
    }
}
impl Render for ArcWorkspaceModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let types = self.config.types.clone();
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            for (name, path) in types {
                let this = this.clone();
                menu = menu.entry(format!("{name} — {path}"), None, move |_, cx| {
                    this.update(cx, |modal, cx| {
                        if !modal.creating {
                            modal.workspace_type = name.clone();
                            modal.error = None;
                            cx.notify();
                        }
                    })
                    .log_err();
                });
            }
            menu
        });
        let relative_path = self
            .config
            .types
            .get(&self.workspace_type)
            .cloned()
            .unwrap_or_default();
        v_flex()
            .elevation_3(cx)
            .w(rems(34.))
            .key_context("ArcWorkspaceModal")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::submit))
            .child(ModalHeader::new().child(Label::new("Create Arc Workspace")))
            .child(
                v_flex()
                    .p_4()
                    .gap_3()
                    .child(Label::new(self.options.display_name()).color(Color::Muted))
                    .child(Label::new("Name"))
                    .child(
                        div()
                            .p_2()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .rounded_md()
                            .child(self.name_editor.clone()),
                    )
                    .child(Label::new("Type"))
                    .child(
                        DropdownMenu::new("arc-workspace-type", self.workspace_type.clone(), menu)
                            .style(DropdownStyle::Outlined),
                    )
                    .child(
                        Label::new(format!(
                            "{}/<name>/{}",
                            self.config.mount_root.trim_end_matches('/'),
                            relative_path
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .when_some(self.error.clone(), |view, error| {
                        view.child(Label::new(error).color(Color::Error))
                    })
                    .when(self.creating, |view| {
                        view.child(
                            Label::new("Mounting Arcadia and opening the remote folder…")
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(
                h_flex()
                    .p_3()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-arc-workspace", "Cancel")
                            .disabled(self.creating)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel(&menu::Cancel, window, cx)
                            })),
                    )
                    .child(
                        Button::new("create-arc-workspace", "Create and Open")
                            .style(ButtonStyle::Filled)
                            .disabled(self.creating)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(&menu::Confirm, window, cx)
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    async fn cancelling_deletion_preserves_the_remote_project(cx: &mut TestAppContext) {
        let app_state = cx.update(workspace::AppState::test);
        let metadata = settings::ArcWorkspace {
            name: "sample".into(),
            workspace_type: "custom".into(),
            mount_path: "/home/remote/arcadia-worktrees/sample".into(),
            project_path: "/home/remote/arcadia-worktrees/sample/custom/project".into(),
        };
        let server = SshConnection {
            host: "arc-host".into(),
            ..Default::default()
        };
        let options = RemoteConnectionOptions::Ssh(server.clone().into());
        cx.update(|cx| {
            arc_workspaces::save_workspace(server, metadata.clone(), app_state.fs.clone(), cx)
        })
        .await
        .expect("settings completion")
        .expect("save workspace");
        cx.run_until_parked();
        let project = project::Project::test(app_state.fs.clone(), [], cx).await;
        let (source, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        let task = cx.update(|window, cx| {
            delete_arc_workspace(&source, options.clone(), metadata.clone(), window, cx)
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        assert!(!task.await.expect("cancel deletion"));
        cx.update(|_, cx| {
            assert_eq!(
                arc_workspaces::managed_workspace(
                    &options,
                    std::path::Path::new(&metadata.project_path),
                    cx
                ),
                Some(metadata.clone())
            );
            assert!(arc_workspaces::begin_deletion(&options, &metadata, cx).is_ok());
        });
    }
}
