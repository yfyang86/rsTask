import { execFile } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import { relative, resolve } from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

export const BOARD_QUERY = `SELECT substr(t.id,1,8) AS id, t.status, t.title, t.updated_at AS updated,
 (SELECT a.name FROM claims c JOIN actors a ON a.id = c.actor_id
   WHERE c.task_id = t.id AND c.released_at IS NULL
     AND c.expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now')
   ORDER BY c.claimed_at DESC LIMIT 1) AS holder,
 (SELECT count(*) FROM task_deps d JOIN tasks b ON b.id = d.blocked_by
   WHERE d.task_id = t.id AND b.status != 'done') AS blockers
FROM tasks t ORDER BY t.updated_at DESC`;

const ORDER = ["work", "review", "queue", "done"];

export function findDatabase(cwd = process.cwd()) {
  const directory = resolve(cwd, ".rstask");
  const databases = readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".sqlite"))
    .map((entry) => {
      const path = resolve(directory, entry.name);
      return { path, mtimeMs: statSync(path).mtimeMs };
    })
    .sort((a, b) => b.mtimeMs - a.mtimeMs);
  if (!databases.length) throw new Error(`No .rstask/*.sqlite board found in ${resolve(cwd)}`);
  return databases[0].path;
}

export async function readBoard({ cwd = process.cwd(), limit = 200 } = {}) {
  const database = findDatabase(cwd);
  const safeLimit = Math.max(1, Math.min(1000, Math.trunc(Number(limit) || 200)));
  const { stdout } = await execFileAsync("sqlite3", ["-readonly", "-json", database, BOARD_QUERY], {
    timeout: 10_000,
    maxBuffer: 16 * 1024 * 1024,
  });
  const parsed = stdout.trim() ? JSON.parse(stdout) : [];
  if (!Array.isArray(parsed)) throw new Error("sqlite3 returned an unexpected board shape");
  const tasks = parsed.slice(0, safeLimit).map((task) => ({
    id: String(task.id),
    status: String(task.status),
    title: String(task.title),
    updated: String(task.updated ?? ""),
    holder: task.holder ? String(task.holder) : null,
    blockers: Number(task.blockers ?? 0),
  }));
  return {
    database: relative(resolve(cwd), database),
    refreshedAt: new Date().toISOString(),
    counts: Object.fromEntries(ORDER.map((status) => [status, tasks.filter((task) => task.status === status).length])),
    tasks,
  };
}

export function formatBoard(board, doneShown = 4) {
  const lines = [];
  for (const status of ORDER) {
    const tasks = board.tasks.filter((task) => task.status === status);
    lines.push(`${status.toUpperCase()} (${tasks.length})`);
    if (!tasks.length) lines.push("  —");
    for (const task of tasks.slice(0, status === "done" ? doneShown : undefined)) {
      const tag = task.holder ? ` @${task.holder}` : task.blockers ? ` ⛔${task.blockers}` : "";
      lines.push(`  ${task.id} ${task.title}${tag}`);
    }
    if (status === "done" && tasks.length > doneShown) lines.push(`  … ${tasks.length - doneShown} more`);
  }
  lines.push(`updated ${board.refreshedAt} · ${board.database}`);
  return lines.join("\n");
}
