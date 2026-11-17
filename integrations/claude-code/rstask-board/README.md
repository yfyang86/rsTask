# rstask-board — Claude Code sidebar pane

A Claude Code plugin (function-hooks mod) that shows the current folder's rstask
board in a pane docked beside the chat: **WORK / REVIEW / QUEUE / DONE**, each task
with its short id and title, the claim holder (`@actor`) for claimed tasks, and
`⛔n` for queued tasks still blocked by `n` unfinished dependencies. It refreshes
every 15 seconds.

## How it reads the board

Read-only, through `sqlite3 -readonly -json` on the most recently modified
`.rstask/*.sqlite` in the session's working directory (rstask's default is
`.rstask/rstask.sqlite`; a workspace may use its own name). It never writes to the
event store and never touches actor keys. Requires `sqlite3` on `PATH`.

## Use

```bash
claude --plugin-dir integrations/claude-code/rstask-board
```

The pane opens by itself where the terminal is wide enough to dock it (about 110–144
columns); `/rstask` opens or refreshes it at any width.

## Files

- `.claude-plugin/plugin.json` — manifest
- `hooks/hooks.json`, `hooks/register.tsx` — the hooks module (session start, `/rstask`, pane rendering)
- `types/index.d.ts` — the `$.state` contract (`board`)
