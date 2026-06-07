from face_mcp import SAMPLE_FPS


def test_default_sample_fps_matches_long_form_benchmark_density() -> None:
    assert SAMPLE_FPS == 0.25
