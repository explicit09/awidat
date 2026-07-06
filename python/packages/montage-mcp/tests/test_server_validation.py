import tempfile
import unittest
from pathlib import Path

from montage_mcp._server import IndexAssetRequest, validate_index_asset_request


def _request(**overrides) -> IndexAssetRequest:
    fields = {
        "asset_path": overrides.pop("asset_path"),
        "asset_id": "media/clip.mp4",
        "asset_sha256": "0" * 64,
    }
    fields.update(overrides)
    return IndexAssetRequest(**fields)


class ValidateIndexAssetRequestTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)
        self.asset = self.root / "clip.mp4"
        self.asset.write_bytes(b"\x00")

    def test_accepts_absolute_existing_regular_file(self) -> None:
        validate_index_asset_request(_request(asset_path=str(self.asset)))

    def test_rejects_relative_asset_path(self) -> None:
        with self.assertRaises(ValueError):
            validate_index_asset_request(_request(asset_path="clip.mp4"))

    def test_rejects_missing_asset_path(self) -> None:
        with self.assertRaises(ValueError):
            validate_index_asset_request(
                _request(asset_path=str(self.root / "nope.mp4"))
            )

    def test_rejects_directory_asset_path(self) -> None:
        with self.assertRaises(ValueError):
            validate_index_asset_request(_request(asset_path=str(self.root)))

    def test_rejects_relative_project_root(self) -> None:
        with self.assertRaises(ValueError):
            validate_index_asset_request(
                _request(asset_path=str(self.asset), project_root="proj")
            )

    def test_rejects_relative_index_root(self) -> None:
        with self.assertRaises(ValueError):
            validate_index_asset_request(
                _request(asset_path=str(self.asset), index_root="idx")
            )

    def test_accepts_absolute_roots(self) -> None:
        validate_index_asset_request(
            _request(
                asset_path=str(self.asset),
                project_root=str(self.root),
                index_root=str(self.root / "index"),
            )
        )


if __name__ == "__main__":
    unittest.main()
