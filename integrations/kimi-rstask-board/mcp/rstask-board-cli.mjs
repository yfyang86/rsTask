#!/usr/bin/env node
import { formatBoard, readBoard } from "./board.mjs";

const args = process.argv.slice(2);
const pathIndex = args.indexOf("--path");
const limitIndex = args.indexOf("--limit");
const path = pathIndex >= 0 ? args[pathIndex + 1] : undefined;
const limit = limitIndex >= 0 ? Number(args[limitIndex + 1]) : undefined;
const board = await readBoard({ cwd: path, limit });
console.log(args.includes("--json") ? JSON.stringify(board, null, 2) : formatBoard(board));
