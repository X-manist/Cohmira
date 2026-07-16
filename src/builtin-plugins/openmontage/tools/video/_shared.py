"""Shared device helper retained for enhancement tools.

Provider-specific video generation helpers were removed. Generated video is
owned by the JiubanAI desktop service and reached through seedance_video.
"""

from __future__ import annotations

import os


def get_torch_device() -> str:
    forced = str(os.environ.get("TORCH_DEVICE") or os.environ.get("OPENMONTAGE_TORCH_DEVICE") or "").strip()
    if forced:
        return forced
    try:
        import torch

        if torch.cuda.is_available():
            return "cuda"
        mps = getattr(torch.backends, "mps", None)
        if mps is not None and mps.is_available():
            return "mps"
    except Exception:
        pass
    return "cpu"
