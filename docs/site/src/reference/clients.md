# Tested MCP clients

The surface is standard MCP — stdio and streamable HTTP — so any
conforming client should work. **"Tested" below means someone actually
ran it against a live Curator and exercised the tools; "untested" means
exactly that and nothing worse.** Dates are the last verification.

| client | stdio | streamable HTTP + bearer |
|---|---|---|
| **Claude Code** | **tested** (2026-07-06) | **tested** (2026-07-06) |
| Claude Desktop | untested | untested |
| MCP Inspector | untested | untested |
| Cursor | untested | untested |
| VS Code (Copilot agent mode) | untested | untested |
| Zed | untested | untested |
| Windsurf | untested | untested |

Tested a client not on this list, or found one that breaks? An issue
with the client, transport, and what happened is exactly the report
this table wants.

## Claude Code

stdio — one command, no token:

```sh
claude mcp add curator -- curator mcp serve --config /path/to/curator.toml
```

Streamable HTTP with a bearer token (a
[server deployment](../operations.md#serving-mcp-over-http)):

```sh
claude mcp add --transport http curator https://your-host/mcp \
  --header "Authorization: Bearer $CURATOR_MCP_TOKEN"
```

Or in a project's `.mcp.json`:

```json
{
  "mcpServers": {
    "curator": {
      "command": "curator",
      "args": ["mcp", "serve", "--config", "/path/to/curator.toml"]
    }
  }
}
```

Verified 2026-07-06 against a live instance: all six tools respond
over both transports; `kp_propose` writes land as open proposals and
wait for `curator review` — [as designed](mcp.md#binding-rules).

## Any other client

Nothing here is Claude-specific. For stdio clients, configure the
command `curator mcp serve` (plus `--config` if the config isn't
discoverable from the working directory). For HTTP clients, point at
the served endpoint and send `Authorization: Bearer <token>`. The
[six tools and their shapes](mcp.md#the-six-tools) are identical over
both transports.
