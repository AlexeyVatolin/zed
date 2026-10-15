use super::*;

impl Editor {
    pub(super) fn copy_arcadia_link_to_trunk(
        &mut self,
        _: &CopyArcadiaLinkToTrunk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_arcadia_link(false, window, cx);
    }

    pub(super) fn copy_arcadia_link_to_current_branch(
        &mut self,
        _: &CopyArcadiaLinkToCurrentBranch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_arcadia_link(true, window, cx);
    }

    fn copy_arcadia_link(
        &mut self,
        current_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = self.arcadia_link(current_branch, cx);
        let workspace = self.workspace();
        cx.spawn_in(window, async move |_, cx| match task.await {
            Ok(url) => {
                cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(url)))
                    .ok();
            }
            Err(error) => {
                let message = format!("Failed to copy Arcadia link: {error:#}");
                log::error!("{message}");
                if let Some(workspace) = workspace {
                    workspace
                        .update_in(cx, |workspace, _, cx| {
                            workspace.show_toast(
                                Toast::new(
                                    NotificationId::unique::<CopyArcadiaLinkToTrunk>(),
                                    message,
                                ),
                                cx,
                            );
                        })
                        .ok();
                }
            }
        })
        .detach();
    }

    fn arcadia_link(&self, current_branch: bool, cx: &mut Context<Self>) -> Task<Result<String>> {
        let result = (|| {
            anyhow::ensure!(
                self.selections
                    .newest_anchor()
                    .head()
                    .diff_base_anchor()
                    .is_none(),
                "cannot link deleted diff lines"
            );
            let selection = self.selections.newest::<Point>(&self.display_snapshot(cx));
            let snapshot = self.buffer.read(cx).snapshot(cx);
            let ranges = snapshot.range_to_buffer_ranges(selection.range());
            anyhow::ensure!(ranges.len() == 1, "selection must be within one file");
            let (buffer_snapshot, range, _) = &ranges[0];
            let buffer = self
                .buffer
                .read(cx)
                .buffer(buffer_snapshot.remote_id())
                .context("selection has no buffer")?;
            let path = buffer
                .read(cx)
                .project_path(cx)
                .context("file is not saved in a project")?;
            let range = range.to_point(buffer_snapshot);
            let end_row = inclusive_end_row(range.start, range.end);
            let project = self.project().context("editor has no project")?;
            Ok(project.update(cx, |project, cx| {
                project.get_arcadia_link(path, range.start.row, end_row, current_branch, cx)
            }))
        })();
        match result {
            Ok(task) => task,
            Err(error) => Task::ready(Err(error)),
        }
    }
}

fn inclusive_end_row(start: Point, end: Point) -> u32 {
    if end.column == 0 && end.row > start.row {
        end.row - 1
    } else {
        end.row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_does_not_include_unselected_next_line() {
        assert_eq!(inclusive_end_row(Point::new(4, 0), Point::new(7, 0)), 6);
        assert_eq!(inclusive_end_row(Point::new(4, 0), Point::new(7, 1)), 7);
        assert_eq!(inclusive_end_row(Point::new(4, 0), Point::new(4, 0)), 4);
    }
}
