# Audio providers

What the app uses for sound effects and music, and the other options we're tracking. Update this when a provider is
added, tried or ruled out. Prices and licenses change often, so check the provider's own page before relying on a
figure here. Last reviewed November 2026.

**Status:** *Integrated* means it works in the app today. *Candidate* means it's worth trying. *Reference* means it's not
something the app calls, but useful alongside it. *Ruled out* means we decided against it, with the reason.

## Integrated

| Provider | Runs | Makes | License for games | Cost | Notes |
|---|---|---|---|---|---|
| **ElevenLabs** (top tier) | Cloud API | Sound effects (0.5–30 s, loopable), music (3 s–10 min, instrumental option) | Commercial use on paid plans (from about $6/month) | Credits from your plan. The app records the credits each request uses. | The best quality and the least setup. The same account covers voices if we add those. Sound effects: `eleven_text_to_sound_v2`/`v3`. Music: `music_v1`, `v2`, `v2_5`. Commercial terms belong to the plan, so check yours. |
| **Stable Audio 3 Small-SFX / Small-Music** | The app's audio server (`audio-server/`), CPU is enough | Sound effects, music (up to 120 s) | Stability AI Community License: free under $1M annual revenue | Free (your hardware) | Runs on this PC without a GPU. The weights are gated: accept the license on Hugging Face first. |
| **Stable Audio 3 Medium** | The app's audio server or ComfyUI, CUDA GPU | Music and sound design, no vocals (up to 380 s) | Same Community License | Free (your hardware) | Needs Flash Attention 2. Reportedly about 6.5 GB VRAM. |
| **ComfyUI workflows** | ComfyUI on this PC, the GPU PC or Lambda | Whatever the workflow makes | The model's license | Free (your hardware) or cloud GPU hours | Any audio workflow exported with *Export (API)*. ComfyUI ships templates for Stable Audio 3, ACE-Step 1.5 and MiniMax Music 3. |

## Open models to run self-hosted (through ComfyUI)

| Model | Makes | License | Hardware | Status |
|---|---|---|---|---|
| **ACE-Step 1.5** | Full songs, vocals or instrumental | MIT (code and weights) | Reportedly 6–8 GB VRAM for the smaller setups; XL needs more | Candidate: probably the most permissive option for music. Native in ComfyUI. |
| **MiniMax Music 3** | Full songs up to 5 min, 32 kHz stereo | Unclear. Sources disagree (a community license, CC BY-NC, CC BY-SA). Read the LICENSE in the Hugging Face repo before shipping anything made with it. | 24 GB GPU (about 22 GB with CPU offload; 8 GB is possible but slow) | Candidate, pending the license check. ComfyUI template `audio_minimax_music_3`. MiniMax's hosted music API closed to new users in August 2026. |
| **DiffRhythm 2** | Songs | Apache 2.0 (reported) | GPU | Candidate |
| **YuE** | Songs with vocals | Apache 2.0 (reported) | Large GPU | Candidate |
| **Magenta RealTime** | Live, steerable music | CC-BY weights (reported) | GPU | Candidate for adaptive music experiments |
| **MusicGen / AudioGen** (Meta AudioCraft) | Music, sound effects | Weights CC BY-NC 4.0 | GPU | Ruled out: non-commercial only |

## Cloud music APIs

| Provider | Notes | Status |
|---|---|---|
| **Stable Audio API** (Stability AI) | Includes Stable Audio 3 Large (API-only). Good for loops and ambience. | Candidate |
| **Mubert** | Music API with game and app licenses (custom plans) | Candidate |
| **Soundraw** | Music API aimed at platforms, games and apps | Candidate |
| **Loudly** | Music API marketed for adaptive, royalty-free soundtracks | Candidate |
| **Suno, Udio** | Popular, but no official public API | Ruled out for integration (manual use only) |

## Free sound libraries (reference)

Often faster than generating, for common sounds like UI clicks, footsteps and coins.

| Library | License | Notes |
|---|---|---|
| [Kenney](https://kenney.nl/assets/category:Audio) | CC0 | UI, impacts, RPG packs |
| Sonniss GDC bundle | Royalty-free, commercial use | A large free bundle every year |
| [Freesound](https://freesound.org) | Varies per file (CC0, CC BY, CC BY-NC) | Check each file. Avoid NC for a commercial game. |

## Things to remember

- **Read the weights' license, not just the code's.** Many repos are MIT or Apache, but their weights are non-commercial.
- **"Royalty-free" isn't a license.** The terms need to cover shipping the audio inside a game you sell.
- **You may not own AI-generated audio.** In the US, purely AI-generated audio may not be protected by copyright. You
  can use it, but you can't stop others from copying it.
