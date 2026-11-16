//! User settings, kept in `settings.json` in the data folder. API keys and server tokens are not stored here (see
//! `secrets.rs`).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub model: String,
    pub n: u32,
    pub aspect_ratio: String,
    pub resolution: String,
    pub quality: String,
    /// The prompt being written, so it's still there after a restart.
    pub prompt: String,
    /// Where new images are saved. `None` means the `images` folder in the data folder.
    pub images_dir: Option<String>,
    /// Where new audio is saved. `None` means the `audio` folder in the data folder.
    pub audio_dir: Option<String>,
    /// Self-hosted audio servers: the app's own audio server and ComfyUI, on this PC, another PC or a cloud GPU.
    pub audio_servers: Vec<AudioServer>,
    /// What 1,000 ElevenLabs credits cost on your plan, for the cost bar.
    pub elevenlabs_usd_per_1k_credits: f64,
    /// Last-used Audio tab choices.
    pub audio: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioServer {
    pub id: String,
    pub name: String,
    /// `audioServer` (this repo's `audio-server`) or `comfyui`.
    pub kind: String,
    /// Base URL, e.g. `http://127.0.0.1:8188`.
    pub url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: "grok-imagine-image-2.0".into(),
            n: 1,
            aspect_ratio: "1:1".into(),
            resolution: "1k".into(),
            quality: "auto".into(),
            prompt: String::new(),
            images_dir: None,
            audio_dir: None,
            audio_servers: vec![],
            // ElevenLabs Creator plan: $22 for 100,000 credits.
            elevenlabs_usd_per_1k_credits: 0.22,
            audio: serde_json::Value::Null,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Writes to a temporary file first so a crash can't leave a half-written settings file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("Couldn't save settings: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("Couldn't save settings: {e}"))
    }
}
