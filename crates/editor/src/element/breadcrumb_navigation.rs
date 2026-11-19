use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, DismissEvent, EntityId, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Task, UniformListScrollHandle, WeakEntity, Window, uniform_list,
};
use project::{DirectoryItem, Project};
use ui::{ButtonLike, PopoverMenu, Tooltip, prelude::*};
use util::paths::PathStyle;
use workspace::{Pane, Workspace};

use crate::Editor;

pub(super) fn render_file_path(
    editor: &Editor,
    editor_id: EntityId,
    dirty: bool,
    cx: &App,
) -> Option<AnyElement> {
    if editor.breadcrumb_header.is_some() {
        return None;
    }
    let project = editor.project()?;
    let workspace = editor.workspace()?;
    let pane = workspace.read(cx).pane_for_item_id(editor_id)?.downgrade();
    let workspace = workspace.downgrade();
    let project_state = project.read(cx);
    if !project_state.is_local() && project_state.remote_client().is_none() {
        return None;
    }
    let buffer = editor.buffer().read(cx).as_singleton()?;
    let file = buffer.read(cx).file()?;
    let file_path = file.file_system_abs_path(cx)?;
    let style = project_state.path_style(cx);
    if !style.is_absolute(&file_path) {
        return None;
    }
    let relative_path = file.path();
    let include_root =
        relative_path.is_empty() || project_state.visible_worktrees(cx).nth(1).is_some();
    let worktree = project_state.worktree_for_id(file.worktree_id(cx), cx)?;
    let worktree = worktree.read(cx);
    let visible_segments = if include_root && !worktree.is_visible() {
        usize::MAX
    } else {
        relative_path.components().count() + usize::from(include_root)
    };
    let mut segments = path_segments(&file_path, style, visible_segments);
    if include_root
        && worktree.is_visible()
        && let Some((label, _, _)) = segments.first_mut()
    {
        *label = worktree.root_name_str().replace('\n', " ");
    }
    if segments.is_empty() {
        return None;
    }

    let mut path = h_flex().gap_1();
    let file_path: Arc<Path> = file_path.into();
    for (index, (label, segment_path, is_file)) in segments.into_iter().enumerate() {
        let id = SharedString::from(format!("breadcrumb:{}", segment_path.display()));
        let (directory, selected) = if is_file {
            (
                style.parent(&segment_path)?.to_path_buf(),
                Some(segment_path),
            )
        } else {
            (segment_path, None)
        };
        let project = project.downgrade();
        let workspace = workspace.clone();
        let pane = pane.clone();
        let copy_path = file_path.clone();
        if index > 0 {
            path = path.child(Label::new("›").color(Color::Placeholder));
        }
        path = path.child(
            PopoverMenu::new(id)
                .trigger(
                    ButtonLike::new(("path-segment", index))
                        .style(ButtonStyle::Transparent)
                        .child(
                            Label::new(label)
                                .buffer_font(cx)
                                .color(if is_file && dirty {
                                    Color::Default
                                } else {
                                    Color::Muted
                                })
                                .when(is_file && dirty, |label| {
                                    label.weight(gpui::FontWeight::BOLD)
                                }),
                        )
                        .on_right_click(move |_, _, cx| {
                            if let Some(path) = copy_path.to_str() {
                                cx.write_to_clipboard(ClipboardItem::new_string(path.to_owned()));
                            }
                        }),
                )
                .menu(move |_, cx| {
                    let project = project.upgrade()?;
                    Some(cx.new(|cx| {
                        let mut menu = DirectoryPopover {
                            project: project.downgrade(),
                            workspace: workspace.clone(),
                            pane: pane.clone(),
                            style,
                            directory: PathBuf::new(),
                            focus: cx.focus_handle(),
                            scroll: UniformListScrollHandle::new(),
                            listing: None,
                            selected: 0,
                            load_directory: Task::ready(()),
                        };
                        menu.load(directory.clone(), selected.clone(), cx);
                        menu
                    }))
                }),
        );
    }
    Some(path.into_any_element())
}

fn path_segments(path: &Path, style: PathStyle, visible: usize) -> Vec<(String, PathBuf, bool)> {
    let mut segments = Vec::new();
    let mut current = Some(path);
    while let Some(path) = current.filter(|_| segments.len() < visible) {
        let label = style.file_name(path).unwrap_or(path.as_os_str());
        segments.push((
            label.to_string_lossy().replace('\n', " "),
            path.to_path_buf(),
            segments.is_empty(),
        ));
        current = style.parent(path);
    }
    segments.reverse();
    segments
}

struct DirectoryPopover {
    project: WeakEntity<Project>,
    workspace: WeakEntity<Workspace>,
    pane: WeakEntity<Pane>,
    style: PathStyle,
    directory: PathBuf,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    listing: Option<anyhow::Result<Vec<DirectoryItem>>>,
    selected: usize,
    load_directory: Task<()>,
}

impl Focusable for DirectoryPopover {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<DismissEvent> for DirectoryPopover {}

impl DirectoryPopover {
    fn load(&mut self, directory: PathBuf, selected: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(project) = self.project.upgrade() else {
            cx.emit(DismissEvent);
            return;
        };
        self.directory = directory;
        self.listing = None;
        self.selected = 0;
        self.scroll = UniformListScrollHandle::new();
        let path = self.directory.to_string_lossy().into_owned();
        let task = project.update(cx, |project, cx| project.list_directory(path, cx));
        let selected = selected
            .as_deref()
            .and_then(|path| self.style.file_name(path))
            .map(PathBuf::from);
        self.load_directory = cx.spawn(async move |this, cx| {
            let listing = task.await;
            let (listing, selected_index) = cx
                .background_spawn(async move {
                    let listing = listing.map(|mut entries| {
                        entries.sort_unstable_by(|a, b| {
                            b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path))
                        });
                        entries
                    });
                    let index = selected.and_then(|selected| {
                        listing
                            .as_ref()
                            .ok()?
                            .iter()
                            .position(|entry| entry.path == selected)
                    });
                    (listing, index)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(index) = selected_index {
                    this.selected = index;
                    this.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
                }
                this.listing = Some(listing);
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn entries(&self) -> &[DirectoryItem] {
        self.listing
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.style.parent(&self.directory) {
            self.load(parent.to_path_buf(), Some(self.directory.clone()), cx);
        }
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries().get(index) else {
            return;
        };
        let path = self.style.join(&self.directory, &entry.path);
        if entry.is_dir {
            self.load(path, None, cx);
            return;
        }
        cx.emit(DismissEvent);
        let (Some(project), Some(workspace)) = (self.project.upgrade(), self.workspace.upgrade())
        else {
            return;
        };
        let pane = self.pane.clone();
        let resolve = Workspace::project_path_for_path(project, &path, false, cx);
        window
            .spawn(cx, async move |cx| {
                let result = async {
                    let (_worktree, path) = resolve.await?;
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            workspace.open_path(path, Some(pane), true, window, cx)
                        })?
                        .await?;
                    anyhow::Ok(())
                }
                .await;
                if let Err(error) = result {
                    workspace.update(cx, |workspace, cx| workspace.show_error(error, cx))?;
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
    }

    fn move_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.entries().len();
        if count == 0 {
            return;
        }
        self.selected = if forward {
            (self.selected + 1).min(count - 1)
        } else {
            self.selected.saturating_sub(1)
        };
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }
}

impl Render for DirectoryPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entry_count = self.entries().len();
        v_flex()
            .id("breadcrumb-directory")
            .track_focus(&self.focus)
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .key_context("menu")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                this.choose(this.selected, window, cx);
            }))
            .on_action(cx.listener(|this, _: &menu::SelectChild, window, cx| {
                if this
                    .entries()
                    .get(this.selected)
                    .is_some_and(|entry| entry.is_dir)
                {
                    this.choose(this.selected, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &menu::SelectParent, _, cx| this.go_up(cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| {
                this.move_selection(false, cx);
            }))
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| {
                this.move_selection(true, cx);
            }))
            .w(rems(23.))
            .p_1()
            .elevation_2(cx)
            .child(
                h_flex()
                    .gap_1()
                    .p_1()
                    .child(
                        IconButton::new("parent", IconName::ArrowUp)
                            .disabled(self.style.parent(&self.directory).is_none())
                            .tooltip(Tooltip::text("Parent Folder"))
                            .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
                    )
                    .child(
                        Label::new(self.directory.to_string_lossy().into_owned()).truncate_start(),
                    ),
            )
            .child(match &self.listing {
                None => div().p_2().child(Label::new("Loading…")).into_any_element(),
                Some(Err(error)) => div()
                    .p_2()
                    .child(
                        Label::new(format!("Could not read folder: {error}")).color(Color::Error),
                    )
                    .into_any_element(),
                Some(Ok(entries)) if entries.is_empty() => div()
                    .p_2()
                    .child(Label::new("Empty folder"))
                    .into_any_element(),
                Some(Ok(_)) => uniform_list(
                    "breadcrumb-directory-entries",
                    entry_count,
                    cx.processor(|this, range, _, cx| {
                        let view = cx.entity().downgrade();
                        range
                            .filter_map(|index| {
                                let item = this.entries().get(index)?;
                                let view = view.clone();
                                let color = cx.theme().colors();
                                Some(
                                    h_flex()
                                        .id(("entry", index))
                                        .h(rems(1.5))
                                        .gap_2()
                                        .px_2()
                                        .cursor_pointer()
                                        .when(this.selected == index, |row| {
                                            row.bg(color.element_selected)
                                        })
                                        .hover(|row| row.bg(color.element_hover))
                                        .child(
                                            Icon::new(if item.is_dir {
                                                IconName::Folder
                                            } else {
                                                IconName::File
                                            })
                                            .size(IconSize::Small),
                                        )
                                        .child(
                                            Label::new(item.path.to_string_lossy().into_owned())
                                                .truncate(),
                                        )
                                        .on_click(move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.choose(index, window, cx)
                                            })
                                            .ok();
                                        }),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .h(rems(entry_count.min(12) as f32 * 1.5))
                .into_any_element(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_targets_use_absolute_paths_even_when_leading_segments_are_hidden() {
        for (style, file, folder) in [
            (
                PathStyle::Unix,
                "/tmp/project/lib/main.rs",
                "/tmp/project/lib",
            ),
            (
                PathStyle::Windows,
                r"C:\project\lib\main.rs",
                r"C:\project\lib",
            ),
        ] {
            assert_eq!(
                path_segments(Path::new(file), style, 2),
                vec![
                    ("lib".into(), PathBuf::from(folder), false),
                    ("main.rs".into(), PathBuf::from(file), true),
                ],
            );
        }
    }
}
