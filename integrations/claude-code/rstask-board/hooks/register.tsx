import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Board, Task } from '../types'

// The board is the session folder's rstasks workspace: the most recently modified
// .rstask/*.sqlite (rstasks' default is rstask.sqlite; a workspace may name its own).
// Read-only access through sqlite3.
const PANE = 'rstask-board'
const REFRESH_MS = 15_000
const ORDER = ['work', 'review', 'queue', 'done'] as const
const DONE_SHOWN = 4

const QUERY = `SELECT substr(t.id,1,8) AS id, t.status, t.title, t.updated_at AS updated,
 (SELECT a.name FROM claims c JOIN actors a ON a.id = c.actor_id
   WHERE c.task_id = t.id AND c.released_at IS NULL
     AND c.expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now')
   ORDER BY c.claimed_at DESC LIMIT 1) AS holder,
 (SELECT count(*) FROM task_deps d JOIN tasks b ON b.id = d.blocked_by
   WHERE d.task_id = t.id AND b.status != 'done') AS blockers
FROM tasks t ORDER BY t.updated_at DESC`

// Change the tag when Board's fields change: it declines what an earlier version stored.
const board = atom({ plugin: 'rstask-board', key: 'board' } as const,
  { tasks: [], refreshedAt: 0, error: null } as Board, { shape: 'board-1' })

const cut = (s: string, n: number) => (s.length <= n ? s : `${s.slice(0, Math.max(0, n - 1))}…`)

async function findDb($: EngineInterface): Promise<string> {
  const dbs = (await $.fs.list('.rstask')).filter(f => f.kind === 'file' && f.name.endsWith('.sqlite'))
  const newest = dbs.sort((a, b) => b.mtimeMs - a.mtimeMs)[0]
  if (!newest) throw new Error('no .rstask/*.sqlite in this folder')
  return `.rstask/${newest.name}`
}

async function refresh($: EngineInterface): Promise<void> {
  try {
    const db = await findDb($)
    const r = await $.process.run(['sqlite3', '-readonly', '-json', db, QUERY], { timeoutMs: 10_000 })
    if (r.exitCode !== 0) throw new Error(r.stderr.trim() || `sqlite3 exit ${r.exitCode}`)
    const tasks = (r.stdout.trim() ? JSON.parse(r.stdout) : []) as Task[]
    await update($, board, () => ({ tasks, refreshedAt: Date.now(), error: null }))
  } catch (err) {
    await update($, board, b => ({ ...b, refreshedAt: Date.now(), error: String(err) }))
  }
  $.ui.invalidate('ui.render')
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.command.register({ name: 'rstask', description: 'Show this folder\'s rstasks kanban board in the sidebar' })
    await refresh($)
    $.clock.every(REFRESH_MS, () => void refresh($))
    void $.ui.open({ id: PANE, title: 'rstasks board' })
    return next(e)
  })

  on('command.run', { command: 'rstask' }, async $ => {
    await refresh($)
    await $.ui.open({ id: PANE, title: 'rstasks board' })
    return { text: 'rstasks board opened (refreshes every 15s).' }
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text } = $.ui.resolve(e)
    const b = await read($, board)
    const width = Math.max(20, (e.props.bodyColumns ?? 40) - 1)
    const by: Record<string, Task[]> = {}
    for (const t of b.tasks) (by[t.status] ??= []).push(t)
    const age = b.refreshedAt ? Math.round((Date.now() - b.refreshedAt) / 1000) : null

    const row = (t: Task) => {
      const tag = t.holder ? ` @${t.holder.replace('claude-reviewer-worker', 'claude')}` : t.blockers ? ` ⛔${t.blockers}` : ''
      return (
        <Text dimColor={t.status === 'done'} color={t.status === 'work' ? 'yellow' : t.blockers ? 'gray' : undefined}>
          {cut(`${t.id.slice(0, 6)} ${t.title}`, width - tag.length)}
          {tag}
        </Text>
      )
    }

    return (
      <Box flexDirection="column">
        {b.error && <Text color="red">{cut(`rstasks: ${b.error}`, width)}</Text>}
        {ORDER.map(s => {
          const list = by[s] ?? []
          const shown = s === 'done' ? list.slice(0, DONE_SHOWN) : list
          return (
            <Box flexDirection="column" marginBottom={1}>
              <Text bold color={s === 'work' ? 'yellow' : s === 'review' ? 'cyan' : s === 'done' ? 'green' : undefined}>
                {s.toUpperCase()} ({list.length})
              </Text>
              {list.length === 0 && <Text dimColor>  —</Text>}
              {shown.map(row)}
              {s === 'done' && list.length > DONE_SHOWN && <Text dimColor>  … {list.length - DONE_SHOWN} more</Text>}
            </Box>
          )
        })}
        <Text dimColor>{age === null ? 'loading…' : `updated ${age}s ago · /rstask to reopen`}</Text>
      </Box>
    )
  })
}
