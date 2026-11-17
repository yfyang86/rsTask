---
name: rstask-board
description: Read and explain the current workspace's RsTasks kanban board without changing tasks, claims, or actor keys
type: prompt
whenToUse: When the user asks for RsTasks status, work/review/queue/done tasks, claim holders, blockers, or a board refresh
---

Use the plugin's `rstask_board` MCP tool as the authoritative source for RsTasks board state. It finds the most recently modified `.rstask/*.sqlite` database in the requested workspace and reads it with `sqlite3 -readonly -json`.

- WORK / REVIEW / QUEUE / DONE are displayed in that order.
- `@actor` identifies the current claim holder.
- `⛔n` identifies the number of unfinished dependencies blocking a queued task.
- Use `/rstask-board:board` for a fresh snapshot; each invocation reads the database again.
- Do not write to the event store, execute task/claim mutations, or access actor keys.
- If no board or `sqlite3` is unavailable, state the error and do not fabricate a board.

The tool returns task rows, counts, refresh time, and the database path. Keep the original short IDs and titles when presenting results.
