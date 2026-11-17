# RsTask Board for Kimi Code

Read-only Kimi Code plugin that shows the current workspace's RsTask kanban board: **WORK / REVIEW / QUEUE / DONE**, claim holders (`@actor`), and unfinished dependency blockers (`⛔n`). It is adapted from `integrations/claude-code/rstask-board` for Kimi's native plugin, MCP, Skill, and slash-command interfaces.

## Behavior

- Finds the most recently modified `.rstask/*.sqlite` in the selected workspace.
- Executes `sqlite3 -readonly -json` with the same task/holder/blocker query as the Claude Code integration.
- Never mutates tasks, claims, dependencies, the event store, or actor keys.
- Requires Node.js 22.19+ and `sqlite3` on `PATH`; no npm runtime dependencies.

## Install

```text
/plugins install /Volumes/fastDev/projects/RsTask/rsTask/integrations/kimi-rstask-board
/reload
```

The plugin is installed per-user under Kimi's managed plugin directory. Reinstall after changing this source directory.

## Use

```text
/rstask-board:board
```

Each invocation reads a fresh snapshot and prints the four board sections, at most four DONE tasks, a refresh timestamp, and the database path. The session-start Skill also documents the board for automatic model use. The MCP tool is `rstask_board` and accepts optional `path` and `limit` arguments.

For a direct terminal view without Kimi:

```bash
node mcp/rstask-board-cli.mjs --path /path/to/workspace
node mcp/rstask-board-cli.mjs --path /path/to/workspace --json
```

## Test

```bash
npm test
```

Tests create temporary SQLite boards and verify database selection, read-only behavior, holder/blocker formatting, MCP initialize/list/call behavior, the direct CLI, and manifest resources. No live RsTask workspace or model call is required.

## Notes

Kimi Code does not currently expose a custom docked TUI pane API. This plugin therefore uses a slash-command/MCP snapshot rather than recreating Claude Code's 15-second sidebar. Use repeated `/rstask-board:board` calls or the direct CLI when another refresh is needed.
