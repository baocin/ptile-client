#!/usr/bin/env python3
"""
Export ALL DapStack tickets across every project.
Uses the MCP SDK's streamablehttp_client transport.
Reads API key from MCP_DAPSTACK_API_KEY env var.
Outputs JSON to stdout.
"""

import asyncio
import json
import os
import sys
from datetime import datetime, timezone

from mcp.client.streamable_http import streamablehttp_client
from mcp import ClientSession

PROJECTS = [
    "ptiles", "preperc", "timeline", "whimper", "dst",
    "Hermes Agent", "meta", "landsearch", "simple_timemachine_viewer",
    "Life", "snac", "steele.red", "nullvec", "gitvet", "gitkracked",
]

SSE_URL = "https://dapstack.co/api/mcp/sse"
MCP_PROTOCOL_VERSION = "2025-03-26"


async def run() -> dict:
    """Export all tickets across all projects."""
    api_key = os.environ.get("MCP_DAPSTACK_API_KEY", "").strip()
    if not api_key:
        raise RuntimeError("MCP_DAPSTACK_API_KEY env var not set")

    headers = {
        "Authorization": f"Bearer {api_key}",
        "mcp-protocol-version": MCP_PROTOCOL_VERSION,
    }

    all_tickets = []

    async with streamablehttp_client(
        url=SSE_URL,
        headers=headers,
        timeout=120,
    ) as (read_stream, write_stream, _):
        async with ClientSession(read_stream, write_stream) as session:
            await session.initialize()

            for proj in PROJECTS:
                try:
                    result = await asyncio.wait_for(
                        session.call_tool("list_tickets", {
                            "projectName": proj,
                            "limit": 500,
                        }),
                        timeout=60,
                    )

                    for c in result.content:
                        if hasattr(c, "text") and c.text:
                            all_tickets.append(c.text)
                except asyncio.TimeoutError:
                    print(f"  {proj}: timeout", file=sys.stderr)
                except Exception as e:
                    print(f"  {proj}: {e}", file=sys.stderr)

    return {
        "exported_at": datetime.now(timezone.utc).isoformat(),
        "project_list": PROJECTS,
        "total_tickets": len(all_tickets),
        "tickets": all_tickets,
    }


async def main():
    try:
        result = await run()
        print(json.dumps(result, indent=2))
    except Exception as e:
        print(f"FATAL: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
