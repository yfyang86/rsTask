---
description: Show the current workspace's read-only RsTasks kanban board
---

Use the plugin's `rstask_board` MCP tool to read the board for the current workspace. If the user supplied a directory as arguments, use that directory as the tool's `path`; otherwise omit `path` to use the current workspace.

Present four sections in this order: WORK, REVIEW, QUEUE, DONE. For each task show its short ID and title, `@actor` when a claim holder is present, and `⛔n` when unfinished dependencies block a queued task. Show at most four DONE tasks and summarize the remainder. Include the refresh timestamp and database path returned by the tool.

Treat the board as read-only evidence. Do not modify tasks, claims, dependencies, actor keys, or the event store. If the tool reports an error, report it plainly without inventing tasks or claiming a board refresh succeeded.

Additional user instructions: $ARGUMENTS
