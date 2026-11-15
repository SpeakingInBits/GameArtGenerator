# Game Art Generator

A Windows desktop app for generating game art with xAI's Grok Imagine image API. You write a prompt, optionally add up to 5 reference images, and generate. The app shows what each generation cost, keeps running totals, and logs every generation with its images.

Built with [Tauri 2](https://tauri.app) (Rust backend, HTML/JS UI). It's installed and kept up to date with [Velopack](https://velopack.io) from this repo's GitHub releases.

## Installing

Download `GameArtGenerator-win-x64-Setup.exe` from the [latest release](https://github.com/SpeakingInBits/GameArtGenerator/releases/latest) and run it. It installs for your Windows account (no admin needed). After that, the app downloads updates in the background and installs them the next time it starts, or right away with **Restart now**.

You need an xAI API key from [console.x.ai](https://console.x.ai). If the `XAI_API_KEY` environment variable is set, the app uses it. Otherwise it asks for the key and saves it in Windows Credential Manager.

## Using it

**Generate tab**

- **Prompt:** describe what you want. Press Ctrl+Enter to generate. You can start more generations while others are still running.
- **Reference images:** add up to 5 by dragging files onto the window, pasting with Ctrl+V or **Paste** (screenshots or files copied in Explorer), clicking **Add…**, or right-clicking any image and choosing *Use as reference image*. They're sent in the order shown, so the prompt can say "image 1", "image 2". Images larger than 2048 px are scaled down before upload.
- **Settings:** model, number of images (1–10), aspect ratio, resolution and quality. Quality only applies to `grok-imagine-image-2.0`; *auto* means low for new images and medium when reference images are used.
- **Estimated cost** updates as you change settings. What gets logged is the exact cost the API reports for each generation.

**Cost bar** (top right): last generation, this session, all time.

**History tab:** every generation, failed ones included, with prompt, model, settings, cost and status. Search by prompt. Select a row to see its reference images and outputs. **Reuse prompt, settings & references** loads them back into the Generate tab.

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
pasted\            images pasted from the clipboard
```

You can send new images to a different folder (your game project's art folder, for example) in **Settings → Images folder**. The API key isn't stored in any of these files.

## Costs

The app gets current prices from xAI each time it starts:

| Model | Per output image | Per reference image* |
|---|---|---|
| `grok-imagine-image` | $0.02 | ~$0.002 |
| `grok-imagine-image-2.0` | $0.04–$0.08 (quality × resolution) | ~$0.01 |

\*xAI doesn't publish the per-reference charge. These figures come from test calls and are only used for the estimate.

## Development

Needs Rust (stable), Node 20+, and Windows (WebView2 ships with Windows 11).

```bash
npm install
npm run dev          # run the app with live reload of the UI
cd src-tauri && cargo test
```

Set `GAME_ART_GENERATOR_DATA_DIR` to keep development data apart from the installed app's.

| Path | What it is |
|---|---|
| `src/` | The UI (`index.html`, `main.js`, `styles.css`), plain HTML and JS with no build step |
| `src-tauri/src/grok.rs` | xAI API client, prices and cost estimate |
| `src-tauri/src/db.rs` | the SQLite generation log |
| `src-tauri/src/paths.rs` | where data is kept (installed, portable, development) |
| `src-tauri/src/secrets.rs` | the API key (environment variable or Credential Manager) |
| `src-tauri/src/updates.rs` | Velopack self-updates from GitHub releases |
| `src-tauri/src/lib.rs` | commands the UI calls |
| `packaging/` | build and release scripts |

## Releasing

Run the **Release** workflow from the Actions tab on `main`, choosing *Bug fixes*, *New features* or *Breaking changes*. That bumps the patch, minor or major version of the latest `vX.Y.Z` tag. The workflow builds the app, packages it with Velopack (`packaging/package.sh`), and publishes a GitHub release with the installer, the portable zip, and the update files that installed copies download.

To build the packages locally (needs `vpk` at the version in `.github/workflows/release.yml`):

```bash
VERSION=0.1.0 bash packaging/package.sh
```
