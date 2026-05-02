"""Sidecar header + body model. Mirrors `crates/proto/src/index.rs`.

The header is the same for every indexer; the body is opaque to the
engine. See `crates/proto/INDEX_SCHEMA.md` for the contract.
"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from pydantic import BaseModel, ConfigDict, Field


class SidecarHeader(BaseModel):
    """Self-describing header on every sidecar."""

    model_config = ConfigDict(populate_by_name=True)

    indexer: str
    """Canonical indexer id; matches the directory name under index/."""

    indexer_version: str
    """Software version of the indexer that produced this file."""

    schema_version: str
    """Version of the body schema. Bumped only when `data` shape changes."""

    asset_id: str
    """Asset this sidecar describes (project-relative path)."""

    asset_sha256: str
    """SHA-256 of the asset bytes at index time."""

    produced_at: datetime
    """When the sidecar was produced (UTC RFC-3339)."""


class Sidecar(BaseModel):
    """Sidecar = flat header fields + a `data` object.

    Serialization is flat: header fields appear at the top level alongside
    a single `data` key. This matches the Rust `IndexSidecar<T>` shape that
    uses `#[serde(flatten)]` on the header.
    """

    indexer: str
    indexer_version: str
    schema_version: str
    asset_id: str
    asset_sha256: str
    produced_at: datetime
    data: dict[str, Any] = Field(default_factory=dict)

    @classmethod
    def build(
        cls,
        *,
        indexer: str,
        indexer_version: str,
        schema_version: str,
        asset_id: str,
        asset_sha256: str,
        data: dict[str, Any],
    ) -> "Sidecar":
        """Convenience constructor; stamps `produced_at = utcnow()`."""
        return cls(
            indexer=indexer,
            indexer_version=indexer_version,
            schema_version=schema_version,
            asset_id=asset_id,
            asset_sha256=asset_sha256,
            produced_at=datetime.now(timezone.utc),
            data=data,
        )
