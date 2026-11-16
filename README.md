# Game Art Generator

A Windows desktop app for generating game art with xAI's Grok Imagine image API. You write a prompt, optionally add up to 5 reference images, and generate. The app shows what each generation cost, keeps running totals, and logs every generation with its images.

Built with [Tauri 2](https://tauri.app) (Rust backend, HTML/JS UI). It's installed and kept up to date with [Velopack](https://velopack.io) from this repo's GitHub releases.

## Installing

Download `GameArtGenerator-win-x64-Setup.exe` from the [latest release](https://github.com/SpeakingInBits/GameArtGenerator/releases/latest) and run it. It installs for your Windows account (no admin needed). After that, the app downloads updates in the background and installs them the next time it starts, or right away with **Restart now**.

You need an xAI API key from [console.x.ai](https://console.x.ai). If the `XAI_API_KEY` environment variable is set, the app uses it. Otherwise it asks for the key and saves it in Windows Credential Manager.

To make 3D models you also need a Tripo API key and credits from [platform.tripo3d.ai](https://platform.tripo3d.ai). Add it in **Settings**, or set `TRIPO_API_KEY`. For audio, add an ElevenLabs key (or set `ELEVENLABS_API_KEY`), a self-hosted server, or both.

## Using it

**Generate tab**

- **Prompt:** describe what you want. Press Ctrl+Enter to generate. You can start more generations while others are still running.
- **Reference images:** add up to 5 by dragging files onto the window, pasting with Ctrl+V or **Paste** (screenshots or files copied in Explorer), clicking **Add…**, or right-clicking any image and choosing *Use as reference image*. They're sent in the order shown, so the prompt can say "image 1", "image 2". Images larger than 2048 px are scaled down before upload.
- **Reference sheet…** (next to the prompt): builds a prompt for a turnaround sheet, which shows one character, creature or item from several angles (front, three-quarter, side, back, top) in a neutral pose, with flat lighting on a plain background. Describe the subject, base it on your reference images, or both. It also sets a wide aspect ratio that fits the views. Sheets like this keep a design consistent across later images, and image-to-3D tools such as Tripo or Meshy work best from them. The dialog remembers your last choices.
- **Settings:** model, number of images (1–10), aspect ratio, resolution and quality. Quality only applies to `grok-imagine-image-2.0`; *auto* means low for new images and medium when reference images are used.
- **Estimated cost** updates as you change settings. What gets logged is the exact cost the API reports for each generation.

**3D models:** right-click any image (or use the preview's button bar) and choose **Make 3D model…**. The app sends it to [Tripo](https://www.tripo3d.ai), which builds a textured model.

- If the image is a reference sheet (views side by side on a plain background), the app cuts it into separate views. You set which side each view shows (Front, Left, Back, Right, or Don't send), and Tripo uses them together. Sheets made with **Reference sheet…** come with these set: front, side as Left and back, with three-quarter and top views left out. Otherwise the image goes as one picture.
- Options: texture (standard, detailed or none), PBR materials, and a low-poly mesh for games. The dialog shows the estimated cost and your Tripo credit balance.
- A job takes a minute or two. Its card in Results shows progress, then the model's preview render with a **3D** badge. Right-click it to open the model or show it in its folder. If you close the app mid-job, it picks the job up again the next time it starts.
- The `.glb` model works in Blender, Godot or Unity, and (converted to FBX) in [Mixamo](https://www.mixamo.com) for humanoid rigging.

**Rig & animate:** right-click a finished 3D model's card (or use its History entry) and choose **Rig & animate…**. Tripo adds a skeleton, for humanoids and other body types alike: four legs, six or eight legs, winged, snake-like, aquatic or something else.

- When the dialog opens, Tripo runs a free check that suggests the body type. You can change it.
- Humanoids can use Mixamo, Unity, Unreal, ActorCore, VRM or Tripo bone names. Mixamo names let you add Mixamo's animations to the rig later.
- Pick up to 5 preset animations. Humanoids have idle, walk, run, jump, slash, shoot, hurt, fall, turn, dive and climb. Four-, six- and eight-legged bodies have a walk, and snake-like and aquatic bodies have a move cycle. Winged and "something else" bodies have none yet. *Animate in place* keeps the root still, so your game moves the character.
- Output is GLB or FBX, saved next to the model as `rigged.glb` and `animated_idle_walk.glb`. Rigging and the animations are two Tripo jobs that run one after the other. Each gets a card and a History entry.

**Audio tab:** sound effects and music.

- **Generate with** lists every source you can use:
  - **ElevenLabs**: sound effects (0.5–30 s, can loop seamlessly) and music (up to 10 minutes, instrumental option). Needs an ElevenLabs API key in Settings, and a paid plan for commercial use.
  - **The app's audio server**: Stable Audio 3, running on this PC (the small models work on a CPU) or a GPU machine.
  - **ComfyUI workflows**: MiniMax Music 3, ACE-Step 1.5, Stable Audio 3 Medium or any audio workflow, on your GPU PC or a cloud GPU such as Lambda.

  Self-hosted servers are added in **Settings → Audio servers**. Setup is in [docs/self-hosting.md](docs/self-hosting.md).
- Set the length, the number of variations (1–4), and whatever the source supports: model, loop, how closely to follow the prompt, things to avoid, a seed.
- **Batch** makes one sound per line, for a list like "sword swing, sword hit, shield block…".
- **Clean up** (on by default) trims silence from the ends, sets the peak to -1 dBFS and saves 16-bit WAV, which every engine imports. Loops aren't trimmed, so they stay seamless.
- Every prompt goes in the list on the right with players for its clips, the ElevenLabs credits it used, and a **Use prompt** button.

ElevenLabs reports usage in credits. The app converts them to dollars with your plan's price per 1,000 credits (Settings), so the cost bar includes them. Self-hosted audio costs nothing per sound, but cloud GPU time is billed by the provider for as long as the server runs. [docs/audio-providers.md](docs/audio-providers.md) tracks the other audio providers and models we've looked at, with their licenses.

**Cost bar** (top right): last generation, this session, all time. Includes 3D models and ElevenLabs audio.

**History tab:** every generation and 3D model job, failed ones included, with prompt, model, settings, cost and status. Search by prompt. Select a row to see its reference images and outputs. **Reuse prompt, settings & references** loads them back into the Generate tab.

## Where data is kept

| Copy | Data folder |
|---|---|
| Installed (Setup.exe) | `%LocalAppData%\SpeakingInBits\GameArtGenerator\` |
| Portable (Portable.zip) | `data\` inside the extracted folder |

Velopack installs the app itself in `%LocalAppData%\GameArtGenerator\` and deletes that folder on uninstall, so the installed app keeps its data in a separate folder. Your history and images survive updates and uninstalling. The portable copy keeps everything with it, so the folder can be moved or copied as a whole.

```
history.db         the generation log (SQLite): prompt, settings, cost, file paths, errors
settings.json      last-used settings and prompt, images folder
images\2026-11-14\20261114-175730-04a7\
    image_1.jpg    generated images
    ref_1.jpg      copies of the reference images that were sent
images\models\2026-11-15\20261115-104512-3fa2\
    model.glb      the 3D model from Tripo
    rigged.glb     the model with a skeleton (Rig & animate)
    animated_idle_walk.glb  the rigged model with its animations
    preview.webp   Tripo's preview render
    view_front.png the images that were sent (or input.jpg for a whole image)
audio\2026-11-15\20261115-130512-9c1e\
    sword_swing_1.wav   sounds, named after the prompt
pasted\            images pasted from the clipboard
views\             views cut from reference sheets
comfy-workflows\   ComfyUI workflows imported for audio
```

You can send new images to a different folder (your game project's art folder, for example) in **Settings → Images folder**. The API keys aren't stored in any of these files.

## Costs

The app gets current prices from xAI each time it starts:

| Model | Per output image | Per reference image* |
|---|---|---|
| `grok-imagine-image` | $0.02 | ~$0.002 |
| `grok-imagine-image-2.0` | $0.04–$0.08 (quality × resolution) | ~$0.01 |

\*xAI doesn't publish the per-reference charge. These figures come from test calls and are only used for the estimate.

Tripo charges in credits ($0.01 each): about 20 for an untextured model, 30 with a standard texture, 40 with a detailed one, plus 10 for a low-poly mesh. Rigging is 25 credits and each animation 10. The rig check is free. The app records the credits each task reports.

## Development

Needs Rust (stable), Node 20+, and Windows (WebView2 ships with Windows 11).

```bash
npm install
npm run dev          # run the app with live reload of the UI
cd src-tauri && cargo test
```

Set `GAME_ART_GENERATOR_DATA_DIR` to keep development data apart from the installed app's.

Tests that need a running audio server or ComfyUI are skipped by default. With `python audio-server/server.py --models demo` running:

```bash
cd src-tauri && AUDIO_SERVER_URL=http://127.0.0.1:8190 cargo test -- --ignored live_audio_server
```

| Path | What it is |
|---|---|
| `src/` | The UI (`index.html`, `main.js`, `styles.css`), plain HTML and JS with no build step |
| `src-tauri/src/grok.rs` | xAI API client, prices and cost estimate |
| `src-tauri/src/tripo.rs` | Tripo 3D API client (upload, image or multiview to model, rig check, rig, animation retarget, task polling) |
| `src-tauri/src/sheet.rs` | cuts a reference sheet into separate views |
| `src-tauri/src/audio.rs` | audio clients: ElevenLabs, the app's audio server, ComfyUI (workflow inputs, queue, results) |
| `src-tauri/src/audio_post.rs` | audio clean-up: decode, trim silence, normalize, write WAV |
| `src-tauri/src/audio_commands.rs` | the Audio tab's commands: sources, servers, workflows, generating, the audio log |
| `audio-server/` | the self-hosted audio server (Python; Stable Audio 3 and demo tones) |
| `docs/` | self-hosting guide and the audio provider list |
| `src-tauri/src/db.rs` | the SQLite log of generations and 3D model jobs |
| `src-tauri/src/paths.rs` | where data is kept (installed, portable, development) |
| `src-tauri/src/secrets.rs` | the xAI and Tripo API keys (environment variables or Credential Manager) |
| `src-tauri/src/updates.rs` | Velopack self-updates from GitHub releases |
| `src-tauri/src/lib.rs` | commands the UI calls |
| `packaging/` | build and release scripts |

## Releasing

Run the **Release** workflow from the Actions tab on `main`, choosing *Bug fixes*, *New features* or *Breaking changes*. That bumps the patch, minor or major version of the latest `vX.Y.Z` tag. The workflow builds the app, packages it with Velopack (`packaging/package.sh`), and publishes a GitHub release with the installer, the portable zip, and the update files that installed copies download.

To build the packages locally (needs `vpk` at the version in `.github/workflows/release.yml`):

```bash
VERSION=0.1.0 bash packaging/package.sh
```
