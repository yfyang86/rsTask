import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");

test("Kimi plugin manifest and all declared resources are valid", () => {
  const manifest = JSON.parse(readFileSync(resolve(root, "kimi.plugin.json"), "utf8"));
  assert.equal(manifest.name, "rstask-board");
  assert.equal(manifest.sessionStart.skill, "rstask-board");
  assert.ok(existsSync(resolve(root, manifest.commands)));
  assert.ok(existsSync(resolve(root, manifest.skills, "rstask-board", "SKILL.md")));
  assert.equal(manifest.mcpServers["rstask-board"].command, "node");
  assert.ok(existsSync(resolve(root, manifest.mcpServers["rstask-board"].args[0])));
  assert.ok(existsSync(resolve(root, "commands", "board.md")));
  const boardSource = readFileSync(resolve(root, "mcp", "board.mjs"), "utf8");
  assert.ok(boardSource.includes('"-readonly"'));
  assert.ok(!boardSource.includes("claude-code"));
});

test("package has no runtime dependencies and exposes offline tests", () => {
  const packageJson = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
  assert.equal(packageJson.private, true);
  assert.equal(packageJson.scripts.test, "node --test test/*.test.mjs");
  assert.equal(packageJson.dependencies, undefined);
});
