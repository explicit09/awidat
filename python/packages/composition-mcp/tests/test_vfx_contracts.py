import unittest

from composition_mcp.vfx_contracts import (
    mask_artifact_profile,
    motion_track_package,
)


class VfxContractsTest(unittest.TestCase):
    def test_motion_track_package_emits_surface_track_seed(self) -> None:
        package = motion_track_package(
            asset_id="asset-1",
            track_id="track-phone",
            initial_box_xyxy=[0.2, 0.3, 0.6, 0.8],
            start_frame=12,
            end_frame=42,
            confidence=0.91,
        )

        self.assertEqual(len(package["tracks"]), 1)
        track = package["tracks"][0]
        self.assertEqual(track["id"], "track-phone")
        self.assertEqual(track["asset_id"], "asset-1")
        self.assertEqual(track["kind"], "surface")
        self.assertEqual(track["coordinate_space"], "normalized")
        self.assertEqual(track["confidence"], 0.91)
        self.assertEqual(track["samples"][0]["frame"], 12)
        self.assertEqual(
            track["samples"][0]["points"],
            [[0.2, 0.3], [0.6, 0.3], [0.6, 0.8], [0.2, 0.8]],
        )
        self.assertEqual(package["producer"]["status"], "planned")

    def test_motion_track_package_rejects_invalid_box(self) -> None:
        with self.assertRaisesRegex(ValueError, "box must be ordered"):
            motion_track_package(
                asset_id="asset-1",
                track_id="bad-track",
                initial_box_xyxy=[0.7, 0.3, 0.6, 0.8],
                start_frame=0,
                end_frame=10,
                confidence=0.5,
            )

    def test_mask_artifact_profile_matches_proto_shape(self) -> None:
        profile = mask_artifact_profile(
            artifact_id="mask-subject-a",
            path="vfx/masks/subject-a/%06d.png",
            object_ids=["subject-a"],
            start_frame=100,
            end_frame=160,
            width=1920,
            height=1080,
        )

        self.assertEqual(profile["id"], "mask-subject-a")
        self.assertEqual(profile["kind"], "per_object_png")
        self.assertEqual(profile["object_ids"], ["subject-a"])
        self.assertEqual(profile["frame_range"]["start_time"]["value"], 100)
        self.assertEqual(profile["frame_range"]["duration"]["value"], 61)
        self.assertEqual(profile["frame_count"], 61)
        self.assertEqual(profile["width"], 1920)
        self.assertEqual(profile["height"], 1080)


if __name__ == "__main__":
    unittest.main()
