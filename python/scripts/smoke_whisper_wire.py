"""Wire-path smoke for whisper-mcp: spawn the real stdio server and call
`index_asset` over MCP, exactly as the Rust engine does.

Usage:
    .venv/bin/python scripts/smoke_whisper_wire.py /abs/path/to/asset.{wav,m4a,mp4,mov}

Verifies, over the actual protocol:
- the server exposes exactly the `index_asset` tool
- malformed requests (relative asset_path) are rejected at the boundary
- the asset transcribes into a well-formed sidecar (header echo, monotonic
  word timestamps, non-empty words)

Backend selection follows the normal auto chain for the environment the
script runs in (Deepgram key -> parakeet -> whisper.cpp -> WhisperX), so on
Apple Silicon with no DEEPGRAM_API_KEY this exercises the parakeet backend.
Model weights download on first use unless HF_HOME points at a warm cache.
This downloads models and runs real ASR — it is a manual smoke, not CI.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

WORKSPACE = Path(__file__).resolve().parents[1]
VENV_BIN = WORKSPACE / ".venv" / "bin"

# Minimal, explicit server env: the venv for whisper-mcp itself, homebrew +
# system paths for ffmpeg/ffprobe. HF_HOME/WHISPER_*/DEEPGRAM_API_KEY pass
# through so the caller controls cache location and backend choice.
SERVER_ENV = {
    "HOME": os.environ["HOME"],
    "PATH": f"{VENV_BIN}:/opt/homebrew/bin:/usr/bin:/bin",
    **{
        k: v
        for k, v in os.environ.items()
        if k == "HF_TOKEN" or k == "HF_HOME" or k == "DEEPGRAM_API_KEY" or k.startswith("WHISPER_")
    },
}


async def main(asset: str) -> None:
    asset_id = f"raw/{os.path.basename(asset)}"
    params = StdioServerParameters(command=str(VENV_BIN / "whisper-mcp"), env=SERVER_ENV)
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()

            tools = await session.list_tools()
            names = [t.name for t in tools.tools]
            assert names == ["index_asset"], names
            print(f"PASS tool list: {names}")

            # Malformed request must be rejected at the boundary.
            bad = await session.call_tool(
                "index_asset",
                {
                    "asset_path": "relative/path.wav",
                    "asset_id": "raw/x.wav",
                    "asset_sha256": "0" * 64,
                },
            )
            assert bad.isError, "relative asset_path was not rejected"
            bad_text = bad.content[0].text
            assert "must be absolute" in bad_text, bad_text
            print("PASS validation rejects relative asset_path")

            result = await session.call_tool(
                "index_asset",
                {
                    "asset_path": asset,
                    "asset_id": asset_id,
                    "asset_sha256": "0" * 64,
                },
                read_timeout_seconds=None,
            )
            assert not result.isError, result.content[0].text[:400]
            sidecar = json.loads(result.content[0].text)

            assert sidecar["indexer"] == "whisper", sidecar["indexer"]
            assert sidecar["asset_id"] == asset_id
            assert sidecar["asset_sha256"] == "0" * 64
            data = sidecar["data"]
            assert data["words"], "no words transcribed"
            starts = [w["start_s"] for w in data["words"]]
            assert starts == sorted(starts), "word starts not monotonic"
            print(
                f"PASS sidecar: model={data['model']} words={len(data['words'])} "
                f"segments={len(data['segments'])} diarized={data['diarized']} "
                f"speakers={[s['id'] for s in data['speakers']]}"
            )
            preview = " ".join(w["text"] for w in data["words"][:12])
            print(f"     transcript head: {preview!r}")

    print("ALL WIRE TESTS PASSED")


if __name__ == "__main__":
    if len(sys.argv) != 2 or not Path(sys.argv[1]).is_absolute():
        sys.exit(f"usage: {sys.argv[0]} /abs/path/to/asset.{{wav,m4a,mp4,mov}}")
    asyncio.run(main(sys.argv[1]))
