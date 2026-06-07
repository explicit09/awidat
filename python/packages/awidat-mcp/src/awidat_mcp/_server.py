"""Thin wrapper over `mcp.server.fastmcp.FastMCP` for indexer servers.

Each indexer instantiates one `IndexerServer`, registers an
`index_asset(asset_path, asset_id, asset_sha256) -> Sidecar` callable, and
calls `.run()`. The wrapper handles:

- Stdio transport selection (the only one Awidat uses today).
- The `index_asset` tool registration with the canonical signature.
- Re-using the asset_sha256 the engine supplied (we don't re-hash —
  trust the engine; it's authoritative).
- Stderr-only logging (stdout is JSON-RPC; never write to it).

Example:

    from awidat_mcp import IndexerServer, Sidecar

    server = IndexerServer(name="whisper", indexer_version="0.1.0",
                           schema_version="1")

    @server.index_asset
    def handle(req):
        # ...do real work...
        return {"words": [], "segments": []}

    if __name__ == "__main__":
        server.run()
"""

from __future__ import annotations

import logging
import sys
from collections.abc import Callable
from typing import Any

from mcp.server.fastmcp import FastMCP
from pydantic import BaseModel

from awidat_mcp._sidecar import Sidecar


class IndexAssetRequest(BaseModel):
    """Arguments to `index_asset` per `INDEX_SCHEMA.md`."""

    project_root: str | None = None
    """Absolute Awidat project root where sidecars and manifest live."""

    asset_path: str
    """Absolute filesystem path to the source asset."""

    asset_id: str
    """Logical id (project-relative path) for the manifest + sidecar header."""

    asset_sha256: str
    """SHA-256 the engine computed; the indexer trusts it."""


# Type alias for the user-supplied work function. Returns the *body* of
# the sidecar (a JSON-shaped dict); the wrapper builds the full Sidecar.
HandlerFn = Callable[[IndexAssetRequest], dict[str, Any]]


class IndexerServer:
    """One MCP server hosting a single `index_asset` tool."""

    def __init__(
        self,
        *,
        name: str,
        indexer_version: str,
        schema_version: str,
    ) -> None:
        self._name = name
        self._indexer_version = indexer_version
        self._schema_version = schema_version
        self._handler: HandlerFn | None = None

        # All log output to stderr. Stdout is JSON-RPC; writing to it
        # corrupts the protocol stream and is the #1 indexer bug.
        logging.basicConfig(
            level=logging.INFO,
            stream=sys.stderr,
            format="%(asctime)s %(name)s %(levelname)s %(message)s",
        )
        self._log = logging.getLogger(name)

        self._mcp = FastMCP(name)

        @self._mcp.tool(
            description=(
                "Read the source asset and return a Sidecar describing the "
                "channel this indexer is responsible for. The engine writes "
                "the returned object to <project>/index/<indexer>/<asset>.json."
            )
        )
        def index_asset(
            asset_path: str,
            asset_id: str,
            asset_sha256: str,
            project_root: str | None = None,
        ) -> dict[str, Any]:
            req = IndexAssetRequest(
                project_root=project_root,
                asset_path=asset_path,
                asset_id=asset_id,
                asset_sha256=asset_sha256,
            )
            if self._handler is None:
                raise RuntimeError(
                    f"{self._name}: no handler registered. "
                    "Decorate one with @server.index_asset."
                )
            self._log.info(
                "indexing asset", extra={"asset_id": asset_id, "path": asset_path}
            )
            data = self._handler(req)
            sc = Sidecar.build(
                indexer=self._name,
                indexer_version=self._indexer_version,
                schema_version=self._schema_version,
                asset_id=asset_id,
                asset_sha256=asset_sha256,
                data=data,
            )
            return sc.model_dump(mode="json")

    def index_asset(self, fn: HandlerFn) -> HandlerFn:
        """Decorator to register the work function. Call once per server."""
        if self._handler is not None:
            raise RuntimeError(f"{self._name}: handler already registered")
        self._handler = fn
        return fn

    def run(self) -> None:
        """Run the MCP stdio server. Blocks until stdin EOF."""
        self._mcp.run(transport="stdio")
