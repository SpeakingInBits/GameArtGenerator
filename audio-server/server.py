"""Game Art Generator audio server.

A small HTTP server that runs open audio models for the Game Art Generator app, on this PC, another PC on your
network, or a cloud GPU (Lambda and similar). See README.md for setup.

    GET  /v1/info       the server and its models
    POST /v1/generate   {model, prompt, negative_prompt?, seconds, seed?, variations} -> {clips: [{format, data}], seed}

Clips come back as base64-encoded 16-bit WAV. If a token is set (--token or AUDIO_SERVER_TOKEN), requests must send
"Authorization: Bearer <token>".
"""

from __future__ import annotations

import argparse
import base64
import hmac
import os
import random
import threading
import time

import uvicorn
from fastapi import Depends, FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

from backends import Backend, available_backends

VERSION = "1.0.0"
MAX_VARIATIONS = 4


class GenerateRequest(BaseModel):
    model: str
    prompt: str = Field(min_length=1, max_length=2000)
    negative_prompt: str | None = None
    seconds: float = Field(gt=0, le=600)
    seed: int | None = Field(default=None, ge=0, le=2**32 - 1)
    variations: int = Field(default=1, ge=1, le=MAX_VARIATIONS)


def create_app(backends: dict[str, Backend], token: str | None, name: str) -> FastAPI:
    app = FastAPI(title="Game Art Generator audio server", version=VERSION)
    # One generation at a time: models share the GPU (or CPU), and queuing is fairer than thrashing memory.
    lock = threading.Lock()

    def check_token(request: Request) -> None:
        if not token:
            return
        sent = request.headers.get("authorization", "")
        if not hmac.compare_digest(sent, f"Bearer {token}"):
            raise HTTPException(status_code=401, detail="Wrong or missing token.")

    @app.exception_handler(HTTPException)
    async def http_error(_: Request, exc: HTTPException) -> JSONResponse:
        return JSONResponse(status_code=exc.status_code, content={"error": str(exc.detail)})

    @app.get("/v1/info", dependencies=[Depends(check_token)])
    def info() -> dict:
        models = []
        for backend in backends.values():
            models.extend(m.to_json() for m in backend.models())
        return {"name": name, "version": VERSION, "models": models}

    @app.post("/v1/generate", dependencies=[Depends(check_token)])
    def generate(req: GenerateRequest) -> dict:
        backend = next((b for b in backends.values() if any(m.id == req.model for m in b.models())), None)
        if backend is None:
            raise HTTPException(status_code=404, detail=f"This server doesn't have the model {req.model}.")
        model = next(m for m in backend.models() if m.id == req.model)
        seconds = min(req.seconds, model.max_seconds)
        seed = req.seed if req.seed is not None else random.randrange(2**31)
        clips = []
        with lock:
            started = time.monotonic()
            for i in range(req.variations):
                try:
                    wav = backend.generate(req.model, req.prompt, req.negative_prompt, seconds, seed + i)
                except Exception as e:  # report the model's error to the app instead of a bare 500
                    raise HTTPException(status_code=500, detail=f"{type(e).__name__}: {e}") from e
                clips.append({"format": "wav", "data": base64.b64encode(wav).decode("ascii")})
            print(f"{req.model}: {req.variations} x {seconds:g}s in {time.monotonic() - started:.1f}s  {req.prompt[:60]!r}")
        return {"clips": clips, "seed": seed}

    return app


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default="127.0.0.1",
                        help="Address to listen on. The default only accepts this machine (use an SSH tunnel from "
                             "other machines). Use 0.0.0.0 for your local network, with a --token.")
    parser.add_argument("--port", type=int, default=8190)
    parser.add_argument("--models", default="auto",
                        help="Comma-separated: demo, small-sfx, small-music, medium. 'auto' picks the Stable Audio 3 "
                             "models this machine can run (medium needs a CUDA GPU), or demo if Stable Audio 3 "
                             "isn't installed.")
    parser.add_argument("--token", default=os.environ.get("AUDIO_SERVER_TOKEN"),
                        help="Require this token (or set AUDIO_SERVER_TOKEN).")
    parser.add_argument("--name", default=os.environ.get("AUDIO_SERVER_NAME", "Game Art audio server"))
    args = parser.parse_args()

    if args.host not in ("127.0.0.1", "localhost", "::1") and not args.token:
        print("Warning: listening beyond this machine without a token. Anyone who can reach it can use your GPU.")

    backends = available_backends(args.models)
    for b in backends.values():
        for m in b.models():
            print(f"  {m.id:<12} {m.label} ({m.device}, up to {m.max_seconds:g}s)")
    uvicorn.run(create_app(backends, args.token, args.name), host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
