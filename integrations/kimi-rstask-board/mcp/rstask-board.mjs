import { readBoard } from "./board.mjs";

const SERVER_INFO = { name: "rstask-board", version: "0.1.1" };
let buffer = "";
let chain = Promise.resolve();

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  while (buffer.includes("\n")) {
    const index = buffer.indexOf("\n");
    const line = buffer.slice(0, index).trim();
    buffer = buffer.slice(index + 1);
    if (line) chain = chain.then(() => handle(line)).catch((error) => writeError(null, -32603, error.message));
  }
});
process.stdin.on("end", () => {
  void chain.finally(() => process.exit(0));
});

async function handle(line) {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return writeError(null, -32700, "Invalid JSON-RPC message");
  }
  if (message.id === undefined || message.id === null) return;
  const { id, method, params = {} } = message;
  if (method === "initialize") {
    return writeResult(id, {
      protocolVersion: params.protocolVersion ?? "2024-11-05",
      capabilities: { tools: { listChanged: false } },
      serverInfo: SERVER_INFO,
    });
  }
  if (method === "ping") return writeResult(id, {});
  if (method === "tools/list") {
    return writeResult(id, {
      tools: [{
        name: "rstask_board",
        description: "Read the current workspace's RsTasks board from the newest .rstask/*.sqlite database without modifying it",
        inputSchema: {
          type: "object",
          properties: {
            path: { type: "string", description: "Workspace containing .rstask/; defaults to the MCP server's current working directory" },
            limit: { type: "number", description: "Maximum tasks to return (1–1000, default 200)" },
          },
          additionalProperties: false,
        },
      }],
    });
  }
  if (method === "tools/call") {
    if (params.name !== "rstask_board") return writeError(id, -32602, `Unknown tool ${params.name}`);
    try {
      const args = params.arguments ?? {};
      const board = await readBoard({ cwd: args.path, limit: args.limit });
      return writeResult(id, {
        content: [{ type: "text", text: JSON.stringify(board) }],
        structuredContent: board,
      });
    } catch (error) {
      return writeResult(id, {
        content: [{ type: "text", text: error.message }],
        isError: true,
      });
    }
  }
  if (method === "shutdown") return writeResult(id, null);
  return writeError(id, -32601, `Method ${method} is not supported`);
}

function writeResult(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n");
}

function writeError(id, code, message) {
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } }) + "\n");
}
