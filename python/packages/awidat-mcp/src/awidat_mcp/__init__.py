"""Shared library for Awidat MCP indexers.

What lives here:

- `Sidecar` — Pydantic model of the self-describing sidecar header +
  opaque body. Mirrors the Rust `IndexSidecarHeader` in
  `crates/proto/src/index.rs`. Indexers construct one and return it from
  the `index_asset` MCP tool.
- `asset_sha256` — streaming SHA-256 of a file, identical to the Rust
  implementation in `crates/index/src/sha.rs` so the engine and indexers
  agree on the hash.
- `IndexerServer` — thin wrapper over `mcp.server.fastmcp.FastMCP`. Each
  indexer instantiates one with its name + version + a callable that does
  the actual work, and `IndexerServer.run()` runs the MCP stdio server.

Why a shared library: every indexer (and every v1.5 indexer added later)
re-implements the same JSON-RPC shape, the same sidecar header, the same
hash computation. Without this package they'd each ship a copy and v1.5
indexers would fork from whichever copy looked best when the new author
read the code. With it, "add a new signal" is "import awidat_mcp, write
the body of `index_asset`."
"""

from awidat_mcp._sidecar import Sidecar, SidecarHeader
from awidat_mcp._sha import asset_sha256
from awidat_mcp._server import IndexerServer, IndexAssetRequest

__all__ = [
    "Sidecar",
    "SidecarHeader",
    "asset_sha256",
    "IndexerServer",
    "IndexAssetRequest",
]
