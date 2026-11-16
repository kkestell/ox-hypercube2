// Registers the ACP client's MCP servers, which pi-acp passes in
// PI_ACP_MCP_SERVERS as mcp.json `mcpServers` entries by name.
export default function (pi) {
  const servers = JSON.parse(process.env.PI_ACP_MCP_SERVERS ?? "{}");
  for (const [name, config] of Object.entries(servers)) {
    pi.registerMcpServer(name, config);
  }
}
