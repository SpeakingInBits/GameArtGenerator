# Self-hosted audio: this PC, a GPU PC, or Lambda

The app can generate audio with models running on machines you control, as well as with ElevenLabs. It talks to two
kinds of server:

- **The app's audio server** (`audio-server/` in this repo): a small Python server for models that run well on one
  machine. Today that's Stable Audio 3. Small-SFX and Small-Music even run on a CPU, so this PC can host it. The
  default port is 8190.
- **ComfyUI**: the standard open-source runner for image, audio and video models. Use it for the bigger models
  (MiniMax Music 3, ACE-Step 1.5, Stable Audio 3 Medium). The default port is 8188.

Either kind can run on this PC, another PC on your network, or a rented cloud GPU such as [Lambda](https://lambda.ai).
To the app, every server is just an address. Add each one in **Settings → Audio servers**.

## Keep servers private

Neither server should be open to the internet. ComfyUI has no login at all, so anyone who can reach it can use your
GPU and read its files.

- **On this PC:** keep the default `127.0.0.1` address. Nothing outside the PC can connect.
- **On another machine (a GPU PC or Lambda):** use an **SSH tunnel**. The server keeps listening on its own
  `127.0.0.1`, and SSH forwards a port on this PC to it. The app connects to `http://127.0.0.1:<port>` as if the
  server were local. Windows 10 and 11 include `ssh`.

  ```bash
  ssh -N -L 9188:127.0.0.1:8188 -L 9190:127.0.0.1:8190 user@gpu-pc
  ```

  Leave that window open while you work. In the app, add `http://127.0.0.1:9188` (ComfyUI) and/or
  `http://127.0.0.1:9190` (audio server). Local ports 9188 and 9190 avoid clashing with servers on this PC.
- **On your home network, without SSH:** start the audio server with `--host 0.0.0.0 --token <long random text>`, and
  enter the same token in the app. Don't do this with ComfyUI, which has no token; use SSH for it.

## The app's audio server

### On this PC (CPU, Stable Audio 3 Small)

Needs Python 3.10+, [uv](https://docs.astral.sh/uv/) and Git. Downloads are a few GB.

1. Accept the Stable Audio 3 license on Hugging Face: open
   [stabilityai/stable-audio-3-small-sfx](https://huggingface.co/stabilityai/stable-audio-3-small-sfx) (and the
   small-music model) while signed in, and agree to the terms. Then create a read token under Settings → Access
   Tokens.
2. Install Stable Audio 3 with the CPU build of PyTorch:

   ```bash
   git clone https://github.com/Stability-AI/stable-audio-3
   ```

   ```bash
   cd stable-audio-3
   ```

   ```bash
   uv venv
   ```

   ```bash
   uv pip install torch==2.7.1 torchaudio==2.7.1 --index-url https://download.pytorch.org/whl/cpu
   ```

   ```bash
   uv sync --no-install-package torch --no-install-package torchaudio
   ```

   ```bash
   uv pip install fastapi uvicorn
   ```

3. Sign in to Hugging Face so the weights can download, using the token from step 1:

   ```bash
   uv run huggingface-cli login
   ```

4. Start the server from the `stable-audio-3` folder. Point it at `audio-server/server.py` in your copy of this
   repo, replacing `<repo>` with that folder:

   ```bash
   uv run python "<repo>/audio-server/server.py" --models small-sfx,small-music
   ```

5. In the app, go to **Settings → Audio servers** and add *This PC*, type *Game Art audio server*, address
   `http://127.0.0.1:8190`.

The first sound from each model takes longer, because the weights load (and download the first time).

To check that the app can reach a server before installing any models, run it with the built-in test tones. This
only needs `fastapi` and `uvicorn`:

```bash
python audio-server/server.py --models demo
```

### On a GPU machine (adds Stable Audio 3 Medium)

Same steps, but in step 2 skip the CPU PyTorch install and run `uv sync --inexact`. By default that installs PyTorch
for CUDA 12.6, and keeps Flash Attention, which Medium needs. Start the server with `--models auto` to serve
everything the machine can run.

## ComfyUI

1. Install ComfyUI. On Windows there's ComfyUI Desktop or the portable build; on Linux:

   ```bash
   git clone https://github.com/comfyanonymous/ComfyUI
   ```

   ```bash
   cd ComfyUI && pip install -r requirements.txt
   ```

   ```bash
   python main.py --listen 127.0.0.1 --port 8188
   ```

2. Open ComfyUI in a browser (through the tunnel if it's remote: `http://127.0.0.1:9188`). Go to **Workflow → Browse
   Templates → Audio** and open a template: Stable Audio 3, ACE-Step 1.5 or MiniMax Music 3. ComfyUI offers to
   download the models it needs. Run it once to make sure it works.
3. **Workflow → Export (API)** saves the workflow as JSON.
4. In the app, go to **Settings → Audio servers**:
   - **Add** the ComfyUI server.
   - **Import workflow…** and pick the exported file. The app finds the prompt, length and seed inputs. Check its
     choices, set *Makes* (sound effects or music) and the longest length the model supports, then **Import**.

Each imported workflow appears in the Audio tab under every ComfyUI server you've added, so the same workflow runs on
the GPU PC or on Lambda.

## Lambda (or another GPU cloud)

1. In the Lambda Cloud dashboard, add your SSH public key and launch an instance. A single GPU with 24 GB or more
   (A10, A6000, L40S…) fits MiniMax Music 3. Smaller models fit on less. Lambda's images come with CUDA and PyTorch.
   To keep downloaded models between sessions, attach a persistent filesystem and install into it.
2. Connect with the SSH tunnel from above, using the address Lambda shows:

   ```bash
   ssh -N -L 9188:127.0.0.1:8188 ubuntu@<instance ip>
   ```

3. In a second SSH session, install and start ComfyUI as above, or the audio server (GPU steps). Keep both listening
   on `127.0.0.1`. Lambda's firewall and the tunnel do the rest.
4. Add the server in the app as `http://127.0.0.1:9188`.
5. **When you're done, terminate the instance in the dashboard.** Lambda bills for as long as the instance exists,
   whether or not you're generating. The app doesn't track or stop cloud GPU time.

## Troubleshooting

- **"Can't connect" in the Audio tab:** is the server running? Is the SSH tunnel window still open? Is the address and
  port in Settings right? Settings shows each server's status and error.
- **"Wrong or missing token":** the token in the app doesn't match the one the audio server was started with.
- **ComfyUI "Value not in list":** the workflow names a model file that isn't on that server. Open the workflow in
  ComfyUI on that machine and let it download the models.
- **Out of memory:** pick a smaller model, a shorter length or fewer variations, or a bigger GPU.
