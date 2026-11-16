# Game Art Generator audio server

A small HTTP server that runs open audio models for the app's Audio tab, on this PC, another PC or a cloud GPU. Today
it serves Stable Audio 3 (Small-SFX and Small-Music on CPU, plus Medium on a CUDA GPU), and demo tones for testing.
Bigger models (MiniMax Music 3, ACE-Step 1.5) go through ComfyUI instead.

Setup, SSH tunnels and Lambda are covered in [docs/self-hosting.md](../docs/self-hosting.md).

## Quick start

To check the app can reach it, run it with test tones. This needs only `fastapi` and `uvicorn`:

```bash
python server.py --models demo
```

With Stable Audio 3, run it from inside the `stable-audio-3` environment (see the setup guide):

```bash
uv run python path/to/audio-server/server.py --models auto
```

| Option | Default | |
|---|---|---|
| `--models` | `auto` | `demo`, `small-sfx`, `small-music`, `medium`, comma-separated. `auto` serves the Stable Audio 3 models this machine can run. |
| `--host` | `127.0.0.1` | Only this machine can connect. Reach it from elsewhere through an SSH tunnel, or use `0.0.0.0` with a `--token`. |
| `--port` | `8190` | |
| `--token` | `AUDIO_SERVER_TOKEN` | When set, requests need `Authorization: Bearer <token>`. |
| `--name` | `Game Art audio server` | Shown in the app's Settings. |

## API

`GET /v1/info`:

```json
{ "name": "Game Art audio server", "version": "1.0.0",
  "models": [{ "id": "small-sfx", "label": "Stable Audio 3 Small-SFX", "kind": "sfx", "maxSeconds": 120,
               "device": "cpu", "license": "…", "supportsNegative": false }] }
```

`POST /v1/generate`:

```json
{ "model": "small-sfx", "prompt": "heavy door slam", "negative_prompt": null, "seconds": 2.5, "seed": null, "variations": 2 }
```

The reply is `{"clips": [{"format": "wav", "data": "<base64>"}], "seed": 123}`. Errors look like `{"error": "…"}`.
Requests run one at a time.

## Adding a model

Write a `Backend` subclass in `backends.py` with `models()` and `generate()`, which returns 16-bit WAV bytes, and
register it in `available_backends()`.
