"""Model backends for the audio server.

Each backend lists its models and turns (prompt, length, seed) into a 16-bit WAV file. To add a model family, write
a Backend subclass and register it in available_backends().
"""

from __future__ import annotations

import array
import hashlib
import inspect
import io
import math
import random
import sys
import wave
from dataclasses import dataclass


@dataclass
class ModelInfo:
    id: str
    label: str
    kind: str  # "sfx" or "music"
    max_seconds: float
    device: str  # "cpu" or "cuda"
    license: str | None = None
    supports_negative: bool = False

    def to_json(self) -> dict:
        return {
            "id": self.id,
            "label": self.label,
            "kind": self.kind,
            "maxSeconds": self.max_seconds,
            "device": self.device,
            "license": self.license,
            "supportsNegative": self.supports_negative,
        }


class Backend:
    def models(self) -> list[ModelInfo]:
        raise NotImplementedError

    def generate(self, model: str, prompt: str, negative: str | None, seconds: float, seed: int) -> bytes:
        raise NotImplementedError


def wav_bytes(frames: list[tuple[float, float]] | list[float], rate: int, channels: int) -> bytes:
    """16-bit WAV from samples in -1..1 (interleaved for stereo)."""
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(channels)
        w.setsampwidth(2)
        w.setframerate(rate)
        samples = array.array("h", (int(max(-1.0, min(1.0, s)) * 32767) for s in frames))
        if sys.byteorder == "big":
            samples.byteswap()
        w.writeframes(samples.tobytes())
    return buf.getvalue()


# ---- demo -----------------------------------------------------------------------------------------------------------


class DemoBackend(Backend):
    """Simple synthesized sounds, no downloads or GPU. For checking that the app can reach the server."""

    def models(self) -> list[ModelInfo]:
        return [
            ModelInfo("demo-sfx", "Demo sound effects (test tones)", "sfx", 10, "cpu", "Test only"),
            ModelInfo("demo-music", "Demo music (test arpeggio)", "music", 60, "cpu", "Test only"),
        ]

    def generate(self, model: str, prompt: str, negative: str | None, seconds: float, seed: int) -> bytes:
        rate = 22050
        # The prompt and seed pick the pitch, so different prompts sound different.
        h = int(hashlib.sha256(f"{prompt}|{seed}".encode()).hexdigest()[:8], 16)
        rng = random.Random(h)
        n = int(seconds * rate)
        out: list[float] = []
        if model == "demo-music":
            root = 220 * 2 ** ((h % 12) / 12)
            notes = [1, 5 / 4, 3 / 2, 2, 3 / 2, 5 / 4]
            step = int(rate * 0.25)
            for i in range(n):
                f = root * notes[(i // step) % len(notes)]
                env = math.exp(-3 * (i % step) / step)
                s = 0.3 * env * math.sin(2 * math.pi * f * i / rate)
                out += [s, s]
        else:
            # A short silence, then a pitch sweep with noise that decays: something like a hit or zap.
            start = int(0.05 * rate)
            f0, f1 = 200 + h % 800, 80 + h % 200
            for i in range(n):
                if i < start:
                    out += [0.0, 0.0]
                    continue
                t = (i - start) / rate
                f = f1 + (f0 - f1) * math.exp(-8 * t)
                env = math.exp(-4 * t / max(seconds, 0.1))
                s = env * (0.5 * math.sin(2 * math.pi * f * t) + 0.15 * (rng.random() * 2 - 1))
                out += [s, s * 0.9]
        return wav_bytes(out, rate, 2)


# ---- Stable Audio 3 ---------------------------------------------------------------------------------------------------


class StableAudio3Backend(Backend):
    """Stable Audio 3 (Stability AI): Small-SFX and Small-Music run on CPU; Medium needs a CUDA GPU.

    Needs the stable-audio-3 package (github.com/Stability-AI/stable-audio-3) and access to its gated Hugging Face
    weights. Licensed under the Stability AI Community License: free for organizations under $1M annual revenue.
    """

    LICENSE = "Stability AI Community License (free under $1M revenue)"
    ALL = {
        "small-sfx": ("Stable Audio 3 Small-SFX", "sfx", 120),
        "small-music": ("Stable Audio 3 Small-Music", "music", 120),
        "medium": ("Stable Audio 3 Medium", "music", 380),
    }

    def __init__(self, wanted: list[str], cuda: bool):
        self.cuda = cuda
        self.ids = [m for m in wanted if m in self.ALL and (m != "medium" or cuda)]
        self.loaded: dict[str, object] = {}

    def models(self) -> list[ModelInfo]:
        out = []
        for mid in self.ids:
            label, kind, max_s = self.ALL[mid]
            device = "cuda" if self.cuda else "cpu"
            out.append(ModelInfo(mid, label, kind, max_s, device, self.LICENSE, supports_negative=self._accepts(mid, "negative_prompt")))
        return out

    def _model(self, mid: str):
        if mid not in self.loaded:
            from stable_audio_3 import StableAudioModel  # imported here so the server starts without it

            print(f"Loading {mid} (the first time downloads the weights)...")
            model = StableAudioModel.from_pretrained(mid)
            if self.cuda and hasattr(model, "to"):
                model = model.to("cuda")
            self.loaded[mid] = model
        return self.loaded[mid]

    def _accepts(self, mid: str, param: str) -> bool:
        try:
            from stable_audio_3 import StableAudioModel

            return param in inspect.signature(StableAudioModel.generate).parameters
        except Exception:
            return False

    def generate(self, model: str, prompt: str, negative: str | None, seconds: float, seed: int) -> bytes:
        import numpy as np
        import torch

        m = self._model(model)
        torch.manual_seed(seed)
        kwargs = {"prompt": prompt, "duration": seconds}
        params = inspect.signature(m.generate).parameters
        if "seed" in params:
            kwargs["seed"] = seed
        if negative and "negative_prompt" in params:
            kwargs["negative_prompt"] = negative
        audio = m.generate(**kwargs)

        rate = getattr(m, "sample_rate", None) or getattr(getattr(m, "config", None), "sample_rate", None) or 44100
        if isinstance(audio, (tuple, list)):
            audio = audio[0]
        if hasattr(audio, "detach"):
            audio = audio.detach().float().cpu().numpy()
        audio = np.asarray(audio, dtype=np.float32)
        while audio.ndim > 2:  # drop a batch dimension
            audio = audio[0]
        if audio.ndim == 1:
            audio = audio[None, :]
        if audio.shape[0] > audio.shape[1]:  # (samples, channels) -> (channels, samples)
            audio = audio.T
        peak = float(np.abs(audio).max()) or 1.0
        if peak > 1.0:
            audio = audio / peak
        channels = audio.shape[0]
        pcm = (np.clip(audio.T, -1, 1) * 32767).astype("<i2").tobytes()
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setnchannels(channels)
            w.setsampwidth(2)
            w.setframerate(int(rate))
            w.writeframes(pcm)
        return buf.getvalue()


# ---- choosing backends ------------------------------------------------------------------------------------------------


def _cuda() -> bool:
    try:
        import torch

        return bool(torch.cuda.is_available())
    except Exception:
        return False


def _has_stable_audio_3() -> bool:
    try:
        import stable_audio_3  # noqa: F401

        return True
    except Exception:
        return False


def available_backends(spec: str) -> dict[str, Backend]:
    wanted = [m.strip() for m in spec.split(",") if m.strip()]
    backends: dict[str, Backend] = {}
    if wanted == ["auto"]:
        if _has_stable_audio_3():
            wanted = ["small-sfx", "small-music", "medium"]
        else:
            print("Stable Audio 3 isn't installed, so only the demo models are available. See README.md.")
            wanted = ["demo"]
    if "demo" in wanted:
        backends["demo"] = DemoBackend()
    sa3 = [m for m in wanted if m in StableAudio3Backend.ALL]
    if sa3:
        if not _has_stable_audio_3():
            raise SystemExit("Stable Audio 3 isn't installed in this Python environment. See README.md.")
        cuda = _cuda()
        if "medium" in sa3 and not cuda:
            print("Skipping Stable Audio 3 Medium: it needs a CUDA GPU.")
        backends["stable-audio-3"] = StableAudio3Backend(sa3, cuda)
    if not backends:
        raise SystemExit(f"No models to serve from --models {spec!r}.")
    return backends
