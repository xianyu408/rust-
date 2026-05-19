# MCP integration

The project models MCP configuration in `agent-core::McpSettings`. The current phase stores config shape and server grouping. The next phase should instantiate MCP clients and expose them as Rig tools.

## Example config

See `config/mcp.example.json`.

```json
{
  "mcpServers": {
    "context7": {
      "command": "npx",
      "args": ["-y", "@upstash/context7-mcp"]
    },
    "tavily": {
      "command": "npx",
      "args": ["-y", "tavily-mcp@latest"],
      "env": {
        "TAVILY_API_KEY": "replace-me"
      }
    }
  }
}
```

## Tool trust model

`context7` and internal documentation MCP servers are trusted documentation sources. Tavily and other web search servers are external context sources. External search results cannot be used as pass/fail evidence for a chip design. Pass/fail evidence must come from local EDA commands.

## Rig/rmcp implementation target

Use Rig's `tool::rmcp` support with the official Rust MCP SDK (`rmcp`). The agent should refresh MCP tools during startup, attach trusted documentation tools to the design/review agents, and attach external search tools only to the research agent.

