---
title: Terminal Threads - Zed
description: Run agent CLIs and TUIs directly in terminal-backed threads in Zed.
---

# Terminal Threads

Terminal Threads are terminal-backed threads in the [Threads Sidebar](./parallel-agents.md#threads-sidebar). Use them when you want to run an agent CLI or TUI directly in Zed.

Terminal Threads are different from [External Agents](./external-agents.md). External Agents integrate with Zed through ACP and render as agent threads. Terminal Threads run the native command-line tool in a terminal that Zed organizes as a thread.

## What Zed Owns {#what-zed-owns}

Zed owns the thread surface:

- the terminal-backed thread in the Threads Sidebar
- thread grouping by project
- switching and organizing the terminal session alongside other threads

## What the CLI Owns {#what-the-cli-owns}

The CLI or TUI running inside the terminal owns its own:

- authentication
- model/provider configuration
- subscriptions or API keys
- tool configuration
- skills and instruction files
- MCP configuration

[Zed Agent profiles](./agent-profiles.md), Zed Agent tool permissions, Zed Skills, and Zed Agent MCP settings do not automatically apply to Terminal Threads.

## Opening a Terminal Thread {#opening-a-terminal-thread}

Open the new-thread menu from the [Agent Panel](./agent-panel.md) using the agent selector button on the left or the `+` icon in the top-right of the panel toolbar, then choose **Terminal**. The Terminal Thread opens in the panel body, just like switching to an agent thread.

You can open as many Terminal Threads as you like. Each gets its own entry in the Threads Sidebar.

## Running a Command Automatically {#terminal-thread-init-command}

Regular **Terminal** threads open a plain shell. To choose which agent starts in a new **Herdr Terminal Thread**, open the Settings Editor under **AI** and set **Herdr Default Agent**. The default is `codex`. Or add this to your `settings.json`:

```json [settings]
{
  "agent": {
    "terminal_herdr_default_agent": "claude"
  }
}
```

Set `terminal_herdr_default_agent` to `""` to open a Herdr shell without starting an agent. This setting applies to new Herdr sessions. Reopening a saved thread attaches to its existing session without starting another agent.

For a custom shell command, set `agent.terminal_init_command` in the Settings Editor under **AI**. A nonempty init command overrides the default agent for new Herdr threads. Regular Terminal threads still open a plain shell.

## Persistent Herdr Sessions {#persistent-herdr-sessions}

Choose **Herdr Terminal Thread** from the new thread menu, or press {#kb agent::NewHerdrTerminalThread}. This action always creates a named [Herdr](https://herdr.dev/) session, regardless of which agent type you created last. **Terminal** in the same menu always creates a regular terminal.

On macOS and Linux, if Herdr is missing from the host that runs the terminal, Zed offers to install it. Choose **Install** to run the command from [Herdr's installation guide](https://herdr.dev/docs/install/):

```sh
curl -fsSL https://herdr.dev/install.sh | sh
```

For an SSH project, installation runs on the remote host. Zed opens the requested thread after installation succeeds. Choose **Cancel** to leave Herdr uninstalled. Zed also finds Herdr in the installer's default `~/.local/bin` folder without restarting the window.

To make the default new thread action use Herdr when the last created agent type was a terminal, set:

```json [settings]
{
  "agent": {
    "terminal_herdr_enabled": true,
    "terminal_herdr_session_name_regex": "/arcadia-worktrees/([^/]+)",
    "terminal_herdr_default_agent": "codex"
  }
}
```

The first regex capture group from the current workspace path becomes the session name. Without a match, Zed uses the workspace directory name. If that name is in use, Zed appends `-1`, `-2`, and so on. Names are restricted to 64 ASCII bytes of letters, digits, dots, underscores, and hyphens; other characters become hyphens. Zed shortens a long base name to leave room for a numeric suffix.

To start Herdr with its spaces and agents sidebar hidden, put `sidebar_start_collapsed = true` and `sidebar_collapsed_mode = "hidden"` under `[ui]` in the Herdr `config.toml` on the host running Herdr. Herdr remembers subsequent manual sidebar changes for each session.

Zed launches the configured agent in the new Herdr pane once. When Zed closes, the Herdr server retains the process. Reopening the Terminal Thread reconnects to the same Herdr session and does not repeat the launch. The thread uses a Herdr icon. When a Herdr integration reports a native agent session reference, Zed stores the latest value as you switch conversations inside Codex or Claude. Install the Herdr integration for your agent if you also want Herdr to resume that conversation after a cold Herdr server restart.

When the active agent reports a terminal title, the Terminal Thread shows that conversation title and updates it as you switch conversations. Until a title is available, it shows the Herdr session name. A title you set manually in Zed remains your override.

For example, run `herdr integration install codex` or `herdr integration install claude` on the host where the agent runs. These integrations also let Herdr report the current native session ID when you switch conversations inside one terminal.

Closing or archiving the Terminal Thread in Zed stops and deletes its Herdr session. Zed removes the thread after Herdr confirms deletion. If deletion fails, Zed keeps the thread so you can retry. Herdr must be available on remote hosts as well as local hosts when using remote projects.

## Terminal Thread Titles {#terminal-thread-titles}

The terminal title in the toolbar updates automatically to reflect the running shell or process. You can set a custom name by clicking the title or the pencil icon that appears on hover. In the Threads Sidebar, right-click a Terminal Thread and select **Rename Title**, or select it and press {#kb agent::RenameSelectedThread}.

To edit the title of the active Terminal Thread from the Agent Panel, custom-map {#action agent::RenameSelectedThread} in your `keymap.json`. Its default binding is scoped to the Threads Sidebar.

## Notifications {#terminal-thread-notifications}

When a terminal produces a bell character while not in focus, Zed notifies you the same way it does when an agent finishes: with a visual pop-up and an optional sound. Clicking the notification brings the terminal into focus and clears the indicator.

The same `agent.notify_when_agent_waiting` and `agent.play_sound_when_agent_done` settings apply.

## Closing Terminal Threads {#closing-terminal-threads}

Unlike agent threads, Terminal Threads are closed rather than archived. They do not go to Thread History. To close one, hover over it in the Threads Sidebar and click the **×** button, or select it and press {#kb agent::ArchiveSelectedThread}.

## CLI/TUI Setup Notes {#cli-setup}

Some agent CLIs and TUIs can send terminal signals, such as bell notifications or title updates, that Zed uses to show useful context in the sidebar.

### Claude Code Notifications {#claude-code-notifications}

Claude Code can notify you when it finishes a task or pauses for permission. To enable this, set `preferredNotifChannel` to `"terminal_bell"` in your Claude Code user settings:

```json
{
  "preferredNotifChannel": "terminal_bell"
}
```

You can also set this from within Claude Code by running `/config`, selecting `Local Notifications`, and choosing `Terminal Bell`.

> If you run Claude Code inside tmux, bell notifications may not reach the outer terminal unless passthrough is enabled. Add this to `~/.tmux.conf`:
>
> ```
> set -g allow-passthrough on
> ```

For more, see the [Claude Code documentation](https://code.claude.com/docs/en/terminal-config).

### Amp Notifications {#amp-notifications}

Amp updates terminal titles automatically and can also notify you when it needs your attention. To enable notifications in Zed Terminal Threads, add `AMP_FORCE_BEL=1` to your terminal environment settings:

```json [settings]
{
  "terminal": {
    "env": {
      "AMP_FORCE_BEL": "1"
    }
  }
}
```

Restart Amp after adding the environment variable.

### OpenCode Notifications {#opencode-notifications}

OpenCode can update terminal titles automatically. For Zed notifications, add an OpenCode plugin that emits a terminal bell when OpenCode needs your attention.

Create `.opencode/plugins/zed-bell.js` in your project, or `~/.config/opencode/plugins/zed-bell.js` to use it globally:

```js
export const ZedBell = async () => {
  return {
    event: async ({ event }) => {
      if (process.env.OPENCODE_CLIENT === "acp") return;

      if (event.type === "session.idle" || event.type === "permission.asked") {
        process.stdout.write("\x07");
      }
    },
  };
};
```

Restart OpenCode after adding the plugin.

### Pi Notifications {#pi-notifications}

Pi can use an extension to emit a notification when it finishes a turn. Create `.pi/extensions/zed-bell.ts` in your project, or `~/.pi/agent/extensions/zed-bell.ts` to use it globally:

```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.on("agent_end", async () => {
    process.stdout.write("\x07");
  });
}
```

Restart Pi after adding the extension, or run `/reload` if the extension is in one of Pi's auto-discovered extension locations.

### Codex Terminal Titles {#codex-terminal-titles}

Codex can update the terminal title as it works, which Zed uses to show useful context for Codex Terminal Threads in the sidebar, such as the project, current status, branch, model, or task progress.

To configure this from within Codex, run `/title` and use the picker to choose which fields appear and in what order. Codex saves the selection to `tui.terminal_title` in `~/.codex/config.toml`. You can also edit it directly:

```toml
[tui]
terminal_title = ["spinner", "project-name", "run-state", "thread-title"]
```

## Credentials and Remote Projects {#credentials-and-remote-projects}

Credentials come from the terminal session and the CLI/TUI running inside it.

In remote projects, the CLI may read the remote shell environment and remote config files. In local Terminal Threads, it reads the local shell environment and local config files. Zed does not copy [API keys from LLM provider settings](./use-api-access.md) into Terminal Threads.

## When to Use Terminal Threads {#when-to-use-terminal-threads}

Use Terminal Threads when:

- you want the tool's native CLI/TUI experience
- no ACP integration exists
- you want subscription behavior owned by the CLI
- you want the CLI to use its own native config files

For ACP-integrated agents, see [External Agents](./external-agents.md).
