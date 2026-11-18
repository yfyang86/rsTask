import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import { DatabaseSync } from "node:sqlite";
import { findDatabase, formatBoard, readBoard } from "../mcp/board.mjs";

function fixture(context) {
  const directory = mkdirFixture(context);
  mkdirSync(join(directory, ".rstask"), { recursive: true });
  const database = join(directory, ".rstask", "rstask.sqlite");
  const db = new DatabaseSync(database);
  db.exec(`
    CREATE TABLE tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL, title TEXT NOT NULL, updated_at TEXT NOT NULL, parent_id TEXT);
    CREATE TABLE actors (id TEXT PRIMARY KEY, name TEXT NOT NULL);
    CREATE TABLE claims (task_id TEXT NOT NULL, actor_id TEXT NOT NULL, claimed_at TEXT NOT NULL, released_at TEXT, expires_at TEXT);
    CREATE TABLE task_deps (task_id TEXT NOT NULL, blocked_by TEXT NOT NULL);
    INSERT INTO actors VALUES ('actor-1', 'alice');
    INSERT INTO tasks VALUES ('task-work-123456', 'work', 'Implement board adapter', '2026-11-17T10:00:00Z', NULL);
    INSERT INTO tasks VALUES ('task-review-12345', 'review', 'Review protocol', '2026-11-17T09:00:00Z', NULL);
    INSERT INTO tasks VALUES ('task-queue-12345', 'queue', 'Install plugin', '2026-11-17T08:00:00Z', NULL);
    INSERT INTO tasks VALUES ('task-done-123456', 'done', 'Write SQL query', '2026-11-17T07:00:00Z', NULL);
    INSERT INTO claims VALUES ('task-work-123456', 'actor-1', '2026-11-17T10:00:00Z', NULL, '2099-01-01T00:00:00Z');
    -- As rstasks stores them: a claim that lapsed without anyone sweeping it, and a parent with an open subtask.
    INSERT INTO tasks VALUES ('lapsed-123456789', 'work', 'Abandoned by its holder', '2026-11-17T06:00:00Z', NULL);
    INSERT INTO claims VALUES ('lapsed-123456789', 'actor-1', '2000-01-01T00:00:00.000000000+00:00', NULL, '2000-01-01T00:15:00.000000000+00:00');
    INSERT INTO tasks VALUES ('epic-12345678901', 'queue', 'Epic', '2026-11-17T05:00:00Z', NULL);
    INSERT INTO tasks VALUES ('part-12345678901', 'queue', 'Part of the epic', '2026-11-17T04:00:00Z', 'epic-12345678901');
    INSERT INTO task_deps VALUES ('task-queue-12345', 'task-work-123456');
    INSERT INTO task_deps VALUES ('task-queue-12345', 'task-review-12345');
  `);
  db.close();
  writeFileSync(join(directory, ".rstask", "older.sqlite"), "not a board");
  utimesSync(join(directory, ".rstask", "older.sqlite"), new Date("2020-01-01"), new Date("2020-01-01"));
  return directory;
}

function mkdirFixture(context) {
  const directory = join(tmpdir(), `kimi-rstask-${process.pid}-${Date.now()}-${Math.random().toString(16).slice(2)}`);
  mkdirSync(directory, { recursive: true });
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

test("readBoard finds the newest database, preserves read-only data, and formats holders/blockers", async (context) => {
  const directory = fixture(context);
  assert.equal(findDatabase(directory), join(directory, ".rstask", "rstask.sqlite"));
  const before = createHash("sha256").update(readFileSync(join(directory, ".rstask", "rstask.sqlite"))).digest("hex");
  const board = await readBoard({ cwd: directory });
  const after = createHash("sha256").update(readFileSync(join(directory, ".rstask", "rstask.sqlite"))).digest("hex");
  assert.equal(after, before);
  assert.deepEqual(board.counts, { work: 1, review: 1, queue: 4, done: 1 });
  const lapsed = board.tasks.find((task) => task.title === "Abandoned by its holder");
  assert.deepEqual([lapsed.status, lapsed.holder], ["queue", null]);
  assert.equal(board.tasks.find((task) => task.title === "Epic").blockers, 1);
  const work = board.tasks.find((task) => task.status === "work");
  const queued = board.tasks.find((task) => task.status === "queue");
  assert.equal(work.holder, "alice");
  assert.equal(queued.blockers, 2);
  const formatted = formatBoard(board);
  assert.match(formatted, /WORK \(1\)/);
  assert.match(formatted, /@alice/);
  assert.match(formatted, /⛔2/);
  assert.match(formatted, /DONE \(1\)/);
});

test("MCP server handles initialize, tools/list, tools/call, and unknown methods", async (context) => {
  const directory = fixture(context);
  const pluginRoot = resolve(import.meta.dirname, "..");
  const child = spawn(process.execPath, ["mcp/rstask-board.mjs"], { cwd: pluginRoot, stdio: ["pipe", "pipe", "pipe"] });
  let stdout = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  const messages = [
    { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } },
    { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} },
    { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "rstask_board", arguments: { path: directory } } },
    { jsonrpc: "2.0", id: 4, method: "unknown/method", params: {} },
  ];
  for (const message of messages) child.stdin.write(JSON.stringify(message) + "\n");
  child.stdin.end();
  const timer = setTimeout(() => child.kill("SIGTERM"), 10_000);
  await once(child, "exit");
  clearTimeout(timer);
  const responses = stdout.trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
  assert.equal(responses[0].result.serverInfo.name, "rstask-board");
  assert.equal(responses[1].result.tools[0].name, "rstask_board");
  assert.equal(responses[2].result.structuredContent.counts.work, 1);
  assert.equal(responses[2].result.structuredContent.counts.queue, 4);
  assert.equal(responses[3].error.code, -32601);
});

test("direct CLI returns the same board as JSON", (context) => {
  const directory = fixture(context);
  const pluginRoot = resolve(import.meta.dirname, "..");
  const result = spawnSync(process.execPath, ["mcp/rstask-board-cli.mjs", "--path", directory, "--json"], {
    cwd: pluginRoot,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  const board = JSON.parse(result.stdout);
  assert.equal(board.counts.work, 1);
  assert.equal(board.counts.queue, 4);
});
