//! Audio generation clients.
//!
//! * **ElevenLabs** (cloud, the top tier): `POST /v1/sound-generation` for sound effects and `POST /v1/music` for
//!   music. Both return the audio file itself. Usage is read from `GET /v1/user/subscription` before and after.
//! * **The app's own audio server** (`audio-server/` in this repo), self-hosted on this PC, another PC or a cloud GPU:
//!   `GET /v1/info` lists its models, `POST /v1/generate` returns clips as base64.
//! * **ComfyUI**, self-hosted the same way: a workflow exported with *Export (API)* is filled in (prompt, length,
//!   seed), queued with `POST /prompt`, polled with `GET /history/{id}`, and its audio downloaded with `GET /view`.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ELEVENLABS_BASE: &str = "https://api.elevenlabs.io/v1";
/// Music can take minutes to compose.
const LONG_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SHORT_TIMEOUT: Duration = Duration::from_secs(20);
pub const AUDIO_EXTENSIONS: [&str; 7] = ["wav", "mp3", "flac", "ogg", "opus", "m4a", "aac"];

/// One generated clip, as the provider returned it.
#[derive(Debug)]
pub struct Clip {
    pub bytes: Vec<u8>,
    /// File extension: `mp3`, `wav`, `flac`…
    pub ext: String,
}

fn http(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("GameArtGenerator/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("HTTP client")
}

async fn read_error(service: &str, resp: reqwest::Response) -> String {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    format!("{service} error {status}: {}", error_message(&text))
}

/// Pulls a readable message out of the error shapes these services use.
fn error_message(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return body.chars().take(500).collect();
    };
    // ElevenLabs: {"detail": {"status": "...", "message": "..."}} or {"detail": [{"msg": "..."}]}
    if let Some(m) = v["detail"]["message"].as_str() {
        return m.to_string();
    }
    if let Some(list) = v["detail"].as_array() {
        let msgs: Vec<&str> = list.iter().filter_map(|d| d["msg"].as_str()).collect();
        if !msgs.is_empty() {
            return msgs.join("; ");
        }
    }
    if let Some(m) = v["detail"].as_str() {
        return m.to_string();
    }
    // ComfyUI: {"error": {"message": "...", "details": "..."}, "node_errors": {...}}
    if let Some(m) = v["error"]["message"].as_str() {
        let details = v["error"]["details"].as_str().unwrap_or("");
        let nodes: Vec<String> = v["node_errors"]
            .as_object()
            .map(|o| {
                o.values()
                    .flat_map(|n| n["errors"].as_array().cloned().unwrap_or_default())
                    .filter_map(|e| {
                        Some(format!(
                            "{} {}",
                            e["message"].as_str()?,
                            e["details"].as_str().unwrap_or("")
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out = m.to_string();
        if !details.is_empty() {
            out += &format!(": {details}");
        }
        if !nodes.is_empty() {
            out += &format!(" ({})", nodes.join("; ").trim());
        }
        return out;
    }
    // The app's audio server: {"error": "..."}
    if let Some(m) = v["error"].as_str() {
        return m.to_string();
    }
    body.chars().take(500).collect()
}

// ---- ElevenLabs ---------------------------------------------------------------------------------------------------

pub const ELEVENLABS_SFX_MODELS: [&str; 2] = ["eleven_text_to_sound_v2", "eleven_text_to_sound_v3"];
pub const ELEVENLABS_MUSIC_MODELS: [&str; 3] = ["music_v1", "music_v2", "music_v2_5"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub tier: String,
    pub used: f64,
    pub limit: f64,
}

pub struct ElevenLabsClient {
    http: reqwest::Client,
    api_key: String,
}

impl ElevenLabsClient {
    pub fn new(api_key: String) -> Self {
        Self {
            http: http(LONG_TIMEOUT),
            api_key,
        }
    }

    pub async fn subscription(&self) -> Result<Subscription, String> {
        let resp = self
            .http
            .get(format!("{ELEVENLABS_BASE}/user/subscription"))
            .header("xi-api-key", &self.api_key)
            .timeout(SHORT_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach ElevenLabs: {e}"))?;
        if !resp.status().is_success() {
            return Err(read_error("ElevenLabs", resp).await);
        }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(Subscription {
            tier: v["tier"].as_str().unwrap_or("").to_string(),
            used: v["character_count"].as_f64().unwrap_or(0.0),
            limit: v["character_limit"].as_f64().unwrap_or(0.0),
        })
    }

    async fn post_audio(&self, path: &str, body: &Value) -> Result<Clip, String> {
        let resp = self
            .http
            .post(format!("{ELEVENLABS_BASE}/{path}"))
            .query(&[("output_format", "mp3_44100_128")])
            .header("xi-api-key", &self.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach ElevenLabs: {e}"))?;
        if !resp.status().is_success() {
            return Err(read_error("ElevenLabs", resp).await);
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Couldn't download the audio from ElevenLabs: {e}"))?
            .to_vec();
        Ok(Clip {
            bytes,
            ext: "mp3".into(),
        })
    }

    /// `seconds` of `None` lets ElevenLabs pick the length (0.5–30 s otherwise).
    pub async fn sound_effect(
        &self,
        prompt: &str,
        seconds: Option<f64>,
        looping: bool,
        influence: Option<f64>,
        model: &str,
    ) -> Result<Clip, String> {
        let mut body = json!({ "text": prompt, "loop": looping, "model_id": model });
        if let Some(s) = seconds {
            body["duration_seconds"] = json!(s.clamp(0.5, 30.0));
        }
        if let Some(i) = influence {
            body["prompt_influence"] = json!(i.clamp(0.0, 1.0));
        }
        self.post_audio("sound-generation", &body).await
    }

    /// `seconds` of `None` lets ElevenLabs pick the length (3–600 s otherwise).
    pub async fn music(
        &self,
        prompt: &str,
        seconds: Option<f64>,
        instrumental: bool,
        model: &str,
    ) -> Result<Clip, String> {
        let mut body =
            json!({ "prompt": prompt, "force_instrumental": instrumental, "model_id": model });
        if let Some(s) = seconds {
            body["music_length_ms"] = json!((s.clamp(3.0, 600.0) * 1000.0).round() as u64);
        }
        self.post_audio("music", &body).await
    }
}

// ---- the app's own audio server -----------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerModel {
    pub id: String,
    pub label: String,
    /// `sfx` or `music`.
    pub kind: String,
    pub max_seconds: f64,
    #[serde(default)]
    pub license: Option<String>,
    /// `cpu` or `cuda`.
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub supports_negative: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    pub models: Vec<ServerModel>,
}

pub struct AudioServerClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl AudioServerClient {
    pub fn new(url: &str, token: Option<String>) -> Self {
        Self {
            http: http(LONG_TIMEOUT),
            base: url.trim_end_matches('/').to_string(),
            token,
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    pub async fn info(&self) -> Result<ServerInfo, String> {
        let resp = self
            .auth(self.http.get(format!("{}/v1/info", self.base)))
            .timeout(SHORT_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the audio server at {}: {e}", self.base))?;
        if !resp.status().is_success() {
            return Err(read_error("Audio server", resp).await);
        }
        resp.json()
            .await
            .map_err(|e| format!("The audio server's reply wasn't understood: {e}"))
    }

    pub async fn generate(
        &self,
        model: &str,
        prompt: &str,
        negative: Option<&str>,
        seconds: f64,
        seed: Option<u64>,
        variations: u32,
    ) -> Result<(Vec<Clip>, Option<u64>), String> {
        let body = json!({
            "model": model,
            "prompt": prompt,
            "negative_prompt": negative,
            "seconds": seconds,
            "seed": seed,
            "variations": variations,
        });
        let resp = self
            .auth(self.http.post(format!("{}/v1/generate", self.base)))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the audio server at {}: {e}", self.base))?;
        if !resp.status().is_success() {
            return Err(read_error("Audio server", resp).await);
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("The audio server's reply wasn't understood: {e}"))?;
        parse_server_clips(&v)
    }
}

fn parse_server_clips(v: &Value) -> Result<(Vec<Clip>, Option<u64>), String> {
    let mut clips = Vec::new();
    for c in v["clips"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let data = c["data"]
            .as_str()
            .ok_or("A clip from the audio server has no data.")?;
        let bytes = B64
            .decode(data)
            .map_err(|e| format!("A clip from the audio server couldn't be read: {e}"))?;
        clips.push(Clip {
            bytes,
            ext: c["format"].as_str().unwrap_or("wav").to_lowercase(),
        });
    }
    if clips.is_empty() {
        return Err("The audio server returned no clips.".into());
    }
    Ok((clips, v["seed"].as_u64()))
}

// ---- ComfyUI ------------------------------------------------------------------------------------------------------

/// One input of one node in a workflow, e.g. node "6", input "text".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRef {
    pub node: String,
    pub input: String,
}

/// Which workflow inputs the app fills in. Each role can feed several inputs (some workflows ask for the length
/// in two nodes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ComfyMapping {
    pub prompt: Vec<FieldRef>,
    pub negative: Vec<FieldRef>,
    pub seconds: Vec<FieldRef>,
    pub seed: Vec<FieldRef>,
}

/// A text or number input the user could map, shown when importing a workflow.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldCandidate {
    pub node: String,
    pub input: String,
    /// The node's title in ComfyUI, or its type.
    pub title: String,
    pub value: Value,
}

/// A ComfyUI workflow the app can run, saved in the data folder's `comfy-workflows`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComfyWorkflow {
    pub id: String,
    pub name: String,
    /// `sfx` or `music`.
    pub kind: String,
    pub max_seconds: f64,
    pub mapping: ComfyMapping,
    /// API-format workflow: `{ node id: { class_type, inputs, _meta } }`.
    pub workflow: Value,
}

const TEXT_INPUTS: [&str; 6] = [
    "text",
    "tags",
    "prompt",
    "caption",
    "style",
    "positive_prompt",
];
const SECONDS_INPUTS: [&str; 6] = [
    "seconds",
    "duration",
    "duration_seconds",
    "audio_duration",
    "length_seconds",
    "audio_length",
];
const SEED_INPUTS: [&str; 2] = ["seed", "noise_seed"];

/// Checks the file is an API-format export, not the regular (UI) workflow format.
pub fn validate_workflow(workflow: &Value) -> Result<(), String> {
    if workflow.get("nodes").is_some_and(Value::is_array) {
        return Err(
            "This is a regular ComfyUI workflow file. In ComfyUI, use Workflow → Export (API) and import that file."
                .into(),
        );
    }
    let nodes = workflow
        .as_object()
        .ok_or("This isn't a ComfyUI workflow file.")?;
    if nodes.is_empty() || !nodes.values().all(|n| n["class_type"].is_string()) {
        return Err("This isn't a ComfyUI API workflow (Workflow → Export (API)).".into());
    }
    Ok(())
}

fn node_title(node: &Value) -> String {
    node["_meta"]["title"]
        .as_str()
        .or_else(|| node["class_type"].as_str())
        .unwrap_or("?")
        .to_string()
}

/// Every text or number input, sorted by node id.
pub fn candidates(workflow: &Value) -> Vec<FieldCandidate> {
    let mut out = Vec::new();
    if let Some(nodes) = workflow.as_object() {
        for (id, node) in nodes {
            if let Some(inputs) = node["inputs"].as_object() {
                for (name, value) in inputs {
                    if value.is_string() || value.is_number() {
                        out.push(FieldCandidate {
                            node: id.clone(),
                            input: name.clone(),
                            title: node_title(node),
                            value: value.clone(),
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| {
        let key = |c: &FieldCandidate| (c.node.parse::<u64>().unwrap_or(u64::MAX), c.node.clone());
        key(a).cmp(&key(b)).then(a.input.cmp(&b.input))
    });
    out
}

fn text_field(workflow: &Value, node: &str) -> Option<FieldRef> {
    let inputs = workflow[node]["inputs"].as_object()?;
    TEXT_INPUTS
        .iter()
        .find(|name| inputs.get(**name).is_some_and(Value::is_string))
        .map(|name| FieldRef {
            node: node.to_string(),
            input: name.to_string(),
        })
}

/// Guesses which inputs are the prompt, negative prompt, length and seed. The prompt is the text feeding a sampler's
/// `positive` input (or the first text encoder); the negative prompt feeds `negative`.
pub fn detect_mapping(workflow: &Value) -> ComfyMapping {
    let mut m = ComfyMapping::default();
    let Some(nodes) = workflow.as_object() else {
        return m;
    };
    let mut ids: Vec<&String> = nodes.keys().collect();
    ids.sort_by_key(|id| (id.parse::<u64>().unwrap_or(u64::MAX), (*id).clone()));

    for id in &ids {
        let inputs = &nodes[*id]["inputs"];
        let linked = |name: &str| inputs[name][0].as_str().map(String::from);
        if let (Some(pos), Some(neg)) = (linked("positive"), linked("negative")) {
            if let Some(f) = text_field(workflow, &pos)
                && !m.prompt.contains(&f)
            {
                m.prompt.push(f);
            }
            if let Some(f) = text_field(workflow, &neg)
                && !m.negative.contains(&f)
                && !m.prompt.contains(&f)
            {
                m.negative.push(f);
            }
        }
    }
    if m.prompt.is_empty() {
        let encoder = ids.iter().find(|id| {
            let class = nodes[**id]["class_type"]
                .as_str()
                .unwrap_or("")
                .to_lowercase();
            (class.contains("textencode") || class.contains("prompt"))
                && text_field(workflow, id).is_some()
        });
        if let Some(id) = encoder {
            m.prompt.extend(text_field(workflow, id));
        }
    }
    for id in &ids {
        let Some(inputs) = nodes[*id]["inputs"].as_object() else {
            continue;
        };
        for (name, value) in inputs {
            if !value.is_number() {
                continue;
            }
            let f = FieldRef {
                node: (*id).clone(),
                input: name.clone(),
            };
            if SECONDS_INPUTS.contains(&name.as_str()) {
                m.seconds.push(f);
            } else if SEED_INPUTS.contains(&name.as_str()) {
                m.seed.push(f);
            }
        }
    }
    m
}

/// Fills the mapped inputs. Lengths keep the type the workflow used (whole seconds stay whole).
pub fn fill_workflow(
    workflow: &Value,
    mapping: &ComfyMapping,
    prompt: &str,
    negative: Option<&str>,
    seconds: Option<f64>,
    seed: u64,
) -> Value {
    let mut w = workflow.clone();
    let mut set = |f: &FieldRef, value: Value| {
        if let Some(inputs) = w
            .get_mut(&f.node)
            .and_then(|n| n.get_mut("inputs"))
            .and_then(Value::as_object_mut)
        {
            inputs.insert(f.input.clone(), value);
        }
    };
    for f in &mapping.prompt {
        set(f, json!(prompt));
    }
    if let Some(neg) = negative {
        for f in &mapping.negative {
            set(f, json!(neg));
        }
    }
    if let Some(s) = seconds {
        for f in &mapping.seconds {
            let whole = workflow[&f.node]["inputs"][&f.input].is_i64()
                || workflow[&f.node]["inputs"][&f.input].is_u64();
            set(
                f,
                if whole {
                    json!(s.round() as i64)
                } else {
                    json!(s)
                },
            );
        }
    }
    for f in &mapping.seed {
        set(f, json!(seed));
    }
    w
}

/// An audio file a finished ComfyUI prompt saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComfyFile {
    pub filename: String,
    pub subfolder: String,
    pub kind: String,
}

/// `Ok(None)` while the prompt is still queued or running.
fn parse_history(v: &Value, prompt_id: &str) -> Result<Option<Vec<ComfyFile>>, String> {
    let Some(entry) = v.get(prompt_id) else {
        return Ok(None);
    };
    let status = &entry["status"];
    if status["status_str"].as_str() == Some("error") {
        let detail = status["messages"]
            .as_array()
            .and_then(|msgs| {
                msgs.iter().find_map(|m| {
                    (m[0].as_str() == Some("execution_error")).then(|| {
                        format!(
                            "{} ({})",
                            m[1]["exception_message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .trim(),
                            m[1]["node_type"].as_str().unwrap_or("?")
                        )
                    })
                })
            })
            .unwrap_or_else(|| "the workflow failed".into());
        return Err(format!("ComfyUI: {detail}"));
    }
    if status["completed"].as_bool() == Some(false) {
        return Ok(None);
    }
    let mut files = Vec::new();
    if let Some(outputs) = entry["outputs"].as_object() {
        for out in outputs.values() {
            for list in out.as_object().into_iter().flat_map(|o| o.values()) {
                for f in list.as_array().map(|a| a.as_slice()).unwrap_or_default() {
                    let Some(name) = f["filename"].as_str() else {
                        continue;
                    };
                    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
                    if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                        files.push(ComfyFile {
                            filename: name.to_string(),
                            subfolder: f["subfolder"].as_str().unwrap_or("").to_string(),
                            kind: f["type"].as_str().unwrap_or("output").to_string(),
                        });
                    }
                }
            }
        }
    }
    if files.is_empty() {
        return Err(
            "The workflow finished but saved no audio. Make sure it ends in a Save Audio node."
                .into(),
        );
    }
    Ok(Some(files))
}

pub struct ComfyClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl ComfyClient {
    pub fn new(url: &str, token: Option<String>) -> Self {
        Self {
            http: http(SHORT_TIMEOUT * 6),
            base: url.trim_end_matches('/').to_string(),
            token,
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn get_json(&self, path: &str) -> Result<Value, String> {
        let resp = self
            .auth(self.http.get(format!("{}{path}", self.base)))
            .send()
            .await
            .map_err(|e| format!("Couldn't reach ComfyUI at {}: {e}", self.base))?;
        if !resp.status().is_success() {
            return Err(read_error("ComfyUI", resp).await);
        }
        resp.json()
            .await
            .map_err(|e| format!("ComfyUI's reply wasn't understood: {e}"))
    }

    /// A short description of the machine, e.g. "NVIDIA GeForce RTX 4090 (24 GB)".
    pub async fn describe(&self) -> Result<String, String> {
        let v = self.get_json("/system_stats").await?;
        let device = &v["devices"][0];
        let name = device["name"].as_str().unwrap_or("unknown device");
        let gb = device["vram_total"].as_f64().map(|b| b / 1_073_741_824.0);
        Ok(match gb {
            Some(gb) if gb > 0.5 => format!("{name} ({gb:.0} GB)"),
            _ => name.to_string(),
        })
    }

    /// Runs a filled-in workflow and returns the audio files it saved.
    pub async fn run(&self, workflow: &Value, timeout: Duration) -> Result<Vec<Clip>, String> {
        let client_id = format!("{:016x}", rand::random::<u64>());
        let resp = self
            .auth(self.http.post(format!("{}/prompt", self.base)))
            .json(&json!({ "prompt": workflow, "client_id": client_id }))
            .send()
            .await
            .map_err(|e| format!("Couldn't reach ComfyUI at {}: {e}", self.base))?;
        if !resp.status().is_success() {
            return Err(read_error("ComfyUI", resp).await);
        }
        let queued: Value = resp.json().await.map_err(|e| e.to_string())?;
        let prompt_id = queued["prompt_id"]
            .as_str()
            .ok_or("ComfyUI didn't return a prompt id.")?
            .to_string();

        let started = std::time::Instant::now();
        let files = loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let history = self.get_json(&format!("/history/{prompt_id}")).await?;
            if let Some(files) = parse_history(&history, &prompt_id)? {
                break files;
            }
            if started.elapsed() > timeout {
                return Err(format!(
                    "ComfyUI didn't finish within {} minutes.",
                    timeout.as_secs() / 60
                ));
            }
        };

        let mut clips = Vec::new();
        for f in files {
            let resp = self
                .auth(self.http.get(format!("{}/view", self.base)))
                .query(&[
                    ("filename", f.filename.as_str()),
                    ("subfolder", f.subfolder.as_str()),
                    ("type", f.kind.as_str()),
                ])
                .send()
                .await
                .map_err(|e| format!("Couldn't download {} from ComfyUI: {e}", f.filename))?;
            if !resp.status().is_success() {
                return Err(read_error("ComfyUI", resp).await);
            }
            let bytes = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
            let ext = f
                .filename
                .rsplit('.')
                .next()
                .unwrap_or("flac")
                .to_lowercase();
            clips.push(Clip { bytes, ext });
        }
        Ok(clips)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed-down Stable Audio workflow in ComfyUI's API format.
    fn stable_audio_workflow() -> Value {
        json!({
            "3": { "class_type": "KSampler", "inputs": {
                "seed": 123, "steps": 50, "cfg": 4.98, "sampler_name": "dpmpp_3m_sde", "scheduler": "exponential",
                "denoise": 1.0, "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["11", 0] } },
            "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "stable_audio.safetensors" } },
            "6": { "class_type": "CLIPTextEncode", "_meta": { "title": "Positive" }, "inputs": { "text": "rain", "clip": ["10", 0] } },
            "7": { "class_type": "CLIPTextEncode", "inputs": { "text": "", "clip": ["10", 0] } },
            "11": { "class_type": "EmptyLatentAudio", "inputs": { "seconds": 47.6, "batch_size": 1 } },
            "19": { "class_type": "SaveAudio", "inputs": { "filename_prefix": "audio/ComfyUI", "audio": ["12", 0] } }
        })
    }

    #[test]
    fn detects_and_fills_a_workflow() {
        let w = stable_audio_workflow();
        validate_workflow(&w).unwrap();
        let m = detect_mapping(&w);
        let f = |node: &str, input: &str| FieldRef {
            node: node.into(),
            input: input.into(),
        };
        assert_eq!(m.prompt, [f("6", "text")]);
        assert_eq!(m.negative, [f("7", "text")]);
        assert_eq!(m.seconds, [f("11", "seconds")]);
        assert_eq!(m.seed, [f("3", "seed")]);

        let filled = fill_workflow(&w, &m, "sword clash", Some("music"), Some(2.5), 99);
        assert_eq!(filled["6"]["inputs"]["text"], "sword clash");
        assert_eq!(filled["7"]["inputs"]["text"], "music");
        assert_eq!(filled["11"]["inputs"]["seconds"], 2.5);
        assert_eq!(filled["3"]["inputs"]["seed"], 99);
        // Links are left alone.
        assert_eq!(filled["3"]["inputs"]["positive"], json!(["6", 0]));

        let names: Vec<String> = candidates(&w)
            .iter()
            .map(|c| format!("{}.{}", c.node, c.input))
            .collect();
        assert!(
            names.contains(&"11.seconds".to_string())
                && !names.iter().any(|n| n.ends_with(".positive"))
        );
    }

    #[test]
    fn whole_second_lengths_stay_whole() {
        let w = json!({
            "1": { "class_type": "TextEncodeAceStepAudio", "inputs": { "tags": "battle drums", "lyrics": "[inst]", "duration": 120 } },
            "2": { "class_type": "SaveAudio", "inputs": { "audio": ["1", 0] } }
        });
        let m = detect_mapping(&w);
        assert_eq!(m.prompt[0].input, "tags");
        let filled = fill_workflow(&w, &m, "calm harp", None, Some(45.4), 1);
        assert_eq!(filled["1"]["inputs"]["duration"], 45);
        assert_eq!(filled["1"]["inputs"]["lyrics"], "[inst]");
    }

    #[test]
    fn rejects_ui_workflows() {
        assert!(validate_workflow(&json!({ "nodes": [], "links": [] })).is_err());
        assert!(validate_workflow(&json!([1, 2])).is_err());
    }

    #[test]
    fn reads_comfy_history() {
        assert_eq!(parse_history(&json!({}), "p1").unwrap(), None);
        let done = json!({ "p1": {
            "status": { "status_str": "success", "completed": true },
            "outputs": { "19": { "audio": [{ "filename": "ComfyUI_00001_.flac", "subfolder": "audio", "type": "output" }] },
                         "20": { "images": [{ "filename": "preview.png", "subfolder": "", "type": "temp" }] } } } });
        let files = parse_history(&done, "p1").unwrap().unwrap();
        assert_eq!(
            files,
            [ComfyFile {
                filename: "ComfyUI_00001_.flac".into(),
                subfolder: "audio".into(),
                kind: "output".into()
            }]
        );

        let failed = json!({ "p1": { "status": { "status_str": "error", "completed": false,
            "messages": [["execution_error", { "exception_message": "CUDA out of memory", "node_type": "KSampler" }]] }, "outputs": {} } });
        assert_eq!(
            parse_history(&failed, "p1").unwrap_err(),
            "ComfyUI: CUDA out of memory (KSampler)"
        );
    }

    #[test]
    fn reads_error_shapes() {
        assert_eq!(
            error_message(r#"{"detail":{"status":"quota_exceeded","message":"Out of credits"}}"#),
            "Out of credits"
        );
        assert_eq!(
            error_message(r#"{"detail":[{"msg":"duration too long"}]}"#),
            "duration too long"
        );
        assert_eq!(
            error_message(r#"{"error":"Unknown model"}"#),
            "Unknown model"
        );
        assert!(error_message(r#"{"error":{"message":"Prompt outputs failed validation","details":""},"node_errors":{"4":{"errors":[{"message":"Value not in list","details":"ckpt_name"}]}}}"#)
            .contains("Value not in list ckpt_name"));
    }

    #[test]
    fn reads_server_clips() {
        let v = json!({ "clips": [{ "format": "wav", "data": B64.encode(b"RIFF") }], "seed": 7 });
        let (clips, seed) = parse_server_clips(&v).unwrap();
        assert_eq!(
            (clips[0].bytes.as_slice(), clips[0].ext.as_str(), seed),
            (b"RIFF".as_slice(), "wav", Some(7))
        );
        assert!(parse_server_clips(&json!({ "clips": [] })).is_err());
    }
}
