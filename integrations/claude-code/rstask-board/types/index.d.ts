export type Task = {
  id: string
  status: string
  title: string
  updated: string
  holder: string | null
  blockers: number
}
export type Board = { tasks: Task[]; refreshedAt: number; error: string | null }

declare module 'claude-code' {
  interface PluginState {
    'rstask-board': { board: Shaped<Board> }
  }
}
