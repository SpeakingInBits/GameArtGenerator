//! Client for the Tripo 3D API (v3), which turns images into textured 3D models.
//!
//! * `POST /v3/files`: upload an image, get a `file_token`
//! * `POST /v3/generation/image-to-model`: one image
//! * `POST /v3/generation/multiview-to-model`: front plus left/back/right views
//! * `POST /v3/animations/rig-check`: whether a model can be rigged, and as what (free)
//! * `POST /v3/animations/rig`: adds a skeleton (humanoids and other body types)
//! * `POST /v3/animations/retarget`: applies preset animations to a rigged model
//! * `GET  /v3/tasks/{id}`: poll until `success`, `failed` or `cancelled`; reports `credits_consumed`
//! * `GET  /v3/account/balance`: credits left
//!
//! 1 credit = $0.01. Every response is `{"code": 0, "data": …}`, or a non-zero `code` with `message`.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use image::{ImageFormat, ImageReader, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const API_BASE: &str = "https://openapi.tripo3d.ai/v3";
pub const MODEL_VERSION: &str = "v3.1-20260211";
pub const USD_PER_CREDIT: f64 = 0.01;
/// Tripo takes up to 20 MB; views are downscaled to this longer side, which is plenty for it.
const MAX_UPLOAD_SIDE: u32 = 2048;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Views the multiview endpoint accepts. `front` is required.
pub const VIEWS: [&str; 4] = ["front", "left", "back", "right"];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOptions {
    /// `none`, `standard` or `detailed`.
    pub texture: String,
    /// Base color, metallic, roughness and normal maps (needs a texture).
    pub pbr: bool,
    /// Clean low-poly topology, suited to game assets.
    pub low_poly: bool,
}

impl ModelOptions {
    /// Credits per Tripo's price list. The cost that gets recorded is what the task reports.
    pub fn estimate_credits(&self) -> f64 {
        let mut credits = match self.texture.as_str() {
            "none" => 20.0,
            "detailed" => 40.0,
            _ => 30.0,
        };
        if self.low_poly {
            credits += 10.0;
        }
        credits
    }

    fn apply(&self, body: &mut Value) {
        let textured = self.texture != "none";
        body["texture"] = json!(textured);
        body["pbr"] = json!(textured && self.pbr);
        if textured {
            body["texture_quality"] = json!(if self.texture == "detailed" {
                "detailed"
            } else {
                "standard"
            });
        }
        if self.low_poly {
            body["smart_low_poly"] = json!(true);
        }
    }
}

/// Body types Tripo can rig.
pub const RIG_TYPES: [&str; 8] = [
    "biped",
    "quadruped",
    "hexapod",
    "octopod",
    "avian",
    "serpentine",
    "aquatic",
    "others",
];
/// Bone naming for humanoid (biped) skeletons.
pub const SKELETON_SPECS: [&str; 6] = ["tripo", "mixamo", "unity", "unreal", "actorcore", "vrm"];
/// Tripo takes at most this many animations per retarget task.
pub const MAX_ANIMATIONS: usize = 5;

/// The preset animations Tripo has for a body type (empty if none).
pub fn animations_for(rig_type: &str) -> &'static [&'static str] {
    match rig_type {
        "biped" => &[
            "preset:idle",
            "preset:walk",
            "preset:run",
            "preset:jump",
            "preset:slash",
            "preset:shoot",
            "preset:hurt",
            "preset:fall",
            "preset:turn",
            "preset:dive",
            "preset:climb",
        ],
        "quadruped" => &["preset:quadruped:walk"],
        "hexapod" => &["preset:hexapod:walk"],
        "octopod" => &["preset:octopod:walk"],
        "serpentine" => &["preset:serpentine:march"],
        "aquatic" => &["preset:aquatic:march"],
        _ => &[],
    }
}

/// The rig model for a body type: v3.0 for humanoids (and "others", which only it supports), v2.5 for the rest.
fn rig_model_version(rig_type: &str) -> &'static str {
    match rig_type {
        "biped" | "others" => "v3.0-20260909",
        _ => "v2.5-20260210",
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RigOptions {
    /// One of `RIG_TYPES`.
    pub rig_type: String,
    /// One of `SKELETON_SPECS`; only used for bipeds.
    pub spec: String,
    /// `glb` or `fbx`.
    pub format: String,
    /// Preset ids from `animations_for(rig_type)`, applied after rigging. Up to `MAX_ANIMATIONS`.
    pub animations: Vec<String>,
    /// Animations play without moving the character (for game engines that move it themselves).
    pub in_place: bool,
}

impl RigOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !RIG_TYPES.contains(&self.rig_type.as_str()) {
            return Err(format!("Unknown body type {}.", self.rig_type));
        }
        if self.rig_type == "biped" && !SKELETON_SPECS.contains(&self.spec.as_str()) {
            return Err(format!("Unknown skeleton type {}.", self.spec));
        }
        if !["glb", "fbx"].contains(&self.format.as_str()) {
            return Err(format!("Unknown file format {}.", self.format));
        }
        if self.animations.len() > MAX_ANIMATIONS {
            return Err(format!("Pick at most {MAX_ANIMATIONS} animations."));
        }
        let available = animations_for(&self.rig_type);
        if let Some(a) = self
            .animations
            .iter()
            .find(|a| !available.contains(&a.as_str()))
        {
            return Err(format!("{a} isn't available for this body type."));
        }
        Ok(())
    }

    pub fn rig_credits() -> f64 {
        25.0
    }

    pub fn animation_credits(&self) -> f64 {
        10.0 * self.animations.len() as f64
    }

    fn rig_body(&self, model_task_id: &str) -> Value {
        let mut body = json!({
            "input": model_task_id,
            "model": rig_model_version(&self.rig_type),
            "rig_type": self.rig_type,
            "out_format": self.format,
        });
        if self.rig_type == "biped" {
            body["spec"] = json!(self.spec);
        }
        body
    }

    fn retarget_body(&self, rig_task_id: &str) -> Value {
        json!({
            "input": rig_task_id,
            "animations": self.animations,
            "out_format": self.format,
            "animate_in_place": self.in_place,
        })
    }
}

/// What to build the model from: one image, or named views (`front`, `left`, `back`, `right`) as file tokens.
pub enum TaskInput {
    Image(String),
    Views(Vec<(String, String)>),
}

#[derive(Debug, Clone)]
pub struct TaskStatus {
    /// `queued`, `running`, `success`, `failed`, `cancelled` (or a legacy terminal value).
    pub status: String,
    pub progress: u32,
    pub model_url: Option<String>,
    pub preview_url: Option<String>,
    pub credits: Option<f64>,
    pub error: Option<String>,
    /// Rig check results.
    pub riggable: Option<bool>,
    pub rig_type: Option<String>,
}

impl TaskStatus {
    pub fn is_finished(&self) -> bool {
        !matches!(self.status.as_str(), "queued" | "running")
    }
}

pub struct TripoClient {
    http: reqwest::Client,
    api_key: String,
}

impl TripoClient {
    pub fn new(api_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("GameArtGenerator/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        Self { http, api_key }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, String> {
        let resp = req
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the Tripo API: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Couldn't read the Tripo API's response: {e}"))?;
        let body: Option<Value> = serde_json::from_str(&text).ok();
        match body {
            Some(v) if status.is_success() && v["code"].as_i64() == Some(0) => {
                Ok(v["data"].clone())
            }
            Some(v) => Err(format!(
                "Tripo API error {}: {}",
                status.as_u16(),
                error_message(&v)
            )),
            None => Err(format!(
                "Tripo API error {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            )),
        }
    }

    pub async fn balance(&self) -> Result<f64, String> {
        let data = self
            .send(self.http.get(format!("{API_BASE}/account/balance")))
            .await?;
        data["balance"]
            .as_f64()
            .ok_or_else(|| "Tripo didn't report a balance.".to_string())
    }

    pub async fn upload(&self, path: &Path) -> Result<String, String> {
        let bytes = upload_png(path)?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("view.png")
            .mime_str("image/png")
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let data = self
            .send(self.http.post(format!("{API_BASE}/files")).multipart(form))
            .await?;
        data["file_token"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "Tripo didn't return a file token for the upload.".to_string())
    }

    pub async fn create_task(
        &self,
        input: &TaskInput,
        options: &ModelOptions,
    ) -> Result<String, String> {
        let (endpoint, mut body) = match input {
            TaskInput::Image(token) => ("image-to-model", json!({ "input": token })),
            TaskInput::Views(views) => {
                let inputs: Vec<Value> = views
                    .iter()
                    .map(|(view, token)| json!({ view.as_str(): token }))
                    .collect();
                ("multiview-to-model", json!({ "inputs": inputs }))
            }
        };
        body["model"] = json!(MODEL_VERSION);
        options.apply(&mut body);
        self.start(&format!("generation/{endpoint}"), &body).await
    }

    /// Checks whether a model (by its generation task id) can be rigged. Free.
    pub async fn create_rig_check(&self, model_task_id: &str) -> Result<String, String> {
        self.start("animations/rig-check", &json!({ "input": model_task_id }))
            .await
    }

    pub async fn create_rig(
        &self,
        model_task_id: &str,
        options: &RigOptions,
    ) -> Result<String, String> {
        self.start("animations/rig", &options.rig_body(model_task_id))
            .await
    }

    pub async fn create_retarget(
        &self,
        rig_task_id: &str,
        options: &RigOptions,
    ) -> Result<String, String> {
        self.start("animations/retarget", &options.retarget_body(rig_task_id))
            .await
    }

    /// Starts a task and returns its id.
    async fn start(&self, path: &str, body: &Value) -> Result<String, String> {
        let data = self
            .send(self.http.post(format!("{API_BASE}/{path}")).json(body))
            .await?;
        data["task_id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "Tripo didn't return a task id.".to_string())
    }

    pub async fn task(&self, task_id: &str) -> Result<TaskStatus, String> {
        let data = self
            .send(self.http.get(format!("{API_BASE}/tasks/{task_id}")))
            .await?;
        Ok(parse_task(&data))
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Couldn't download from Tripo: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "Couldn't download from Tripo: HTTP {}",
                resp.status().as_u16()
            ));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Couldn't download from Tripo: {e}"))
    }
}

fn parse_task(data: &Value) -> TaskStatus {
    let output = &data["output"];
    let url = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            output[*k]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
    };
    let error = match (data["error_message"].as_str(), data["error_code"].as_i64()) {
        (Some(m), Some(c)) => Some(format!("{m} (code {c})")),
        (Some(m), None) => Some(m.to_string()),
        (None, Some(c)) => Some(format!("Tripo error code {c}")),
        _ => None,
    };
    TaskStatus {
        status: data["status"].as_str().unwrap_or("unknown").to_string(),
        progress: data["progress"].as_u64().unwrap_or(0).min(100) as u32,
        model_url: url(&["model_url", "pbr_model_url", "base_model_url"]),
        preview_url: url(&["rendered_image_url"]),
        credits: data["credits_consumed"].as_f64(),
        error,
        riggable: output["riggable"].as_bool(),
        rig_type: output["rig_type"].as_str().map(String::from),
    }
}

fn error_message(v: &Value) -> String {
    let msg = v["message"].as_str().unwrap_or("unknown error");
    match v["suggestion"].as_str().filter(|s| !s.is_empty()) {
        Some(s) => format!("{msg}. {s}"),
        None => msg.to_string(),
    }
}

/// The file extension of a model download URL (`glb` unless the URL says otherwise).
pub fn model_extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    if ["glb", "gltf", "fbx", "obj", "usdz", "stl"].contains(&ext.as_str()) && path.contains('.') {
        ext
    } else {
        "glb".into()
    }
}

/// Re-encodes an image as PNG (Tripo takes PNG and JPEG), downscaled if it's very large.
fn upload_png(path: &Path) -> Result<Vec<u8>, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't read {name}: {e}");
    let mut img = ImageReader::open(path)
        .map_err(|e| fail(&e))?
        .with_guessed_format()
        .map_err(|e| fail(&e))?
        .decode()
        .map_err(|e| fail(&e))?;
    if img.width().max(img.height()) > MAX_UPLOAD_SIDE {
        img = img.resize(MAX_UPLOAD_SIDE, MAX_UPLOAD_SIDE, FilterType::Lanczos3);
    }
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| fail(&e))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_follow_the_price_list() {
        let opts = |texture: &str, low_poly| ModelOptions {
            texture: texture.into(),
            pbr: true,
            low_poly,
        };
        assert_eq!(opts("none", false).estimate_credits(), 20.0);
        assert_eq!(opts("standard", false).estimate_credits(), 30.0);
        assert_eq!(opts("detailed", true).estimate_credits(), 50.0);
    }

    #[test]
    fn untextured_models_skip_pbr() {
        let mut body = json!({});
        ModelOptions {
            texture: "none".into(),
            pbr: true,
            low_poly: false,
        }
        .apply(&mut body);
        assert_eq!(body, json!({ "texture": false, "pbr": false }));
    }

    fn rig(rig_type: &str, animations: &[&str]) -> RigOptions {
        RigOptions {
            rig_type: rig_type.into(),
            spec: "mixamo".into(),
            format: "glb".into(),
            animations: animations.iter().map(|a| a.to_string()).collect(),
            in_place: true,
        }
    }

    #[test]
    fn rig_requests_match_the_body_type() {
        let biped = rig("biped", &["preset:idle", "preset:walk"]);
        biped.validate().unwrap();
        assert_eq!(
            biped.rig_body("t1"),
            json!({ "input": "t1", "model": "v3.0-20260909", "rig_type": "biped", "out_format": "glb", "spec": "mixamo" })
        );
        assert_eq!(
            biped.retarget_body("r1"),
            json!({ "input": "r1", "animations": ["preset:idle", "preset:walk"], "out_format": "glb", "animate_in_place": true })
        );
        assert_eq!(biped.animation_credits(), 20.0);

        // Non-humanoids use the v2.5 rig and no skeleton spec.
        let dog = rig("quadruped", &["preset:quadruped:walk"]);
        dog.validate().unwrap();
        assert_eq!(
            dog.rig_body("t1"),
            json!({ "input": "t1", "model": "v2.5-20260210", "rig_type": "quadruped", "out_format": "glb" })
        );
    }

    #[test]
    fn rejects_animations_the_body_type_lacks() {
        assert!(rig("quadruped", &["preset:run"]).validate().is_err());
        assert!(rig("avian", &["preset:quadruped:walk"]).validate().is_err());
        assert!(rig("dragon", &[]).validate().is_err());
        let too_many = rig(
            "biped",
            &[
                "preset:idle",
                "preset:walk",
                "preset:run",
                "preset:jump",
                "preset:slash",
                "preset:hurt",
            ],
        );
        assert!(too_many.validate().is_err());
    }

    #[test]
    fn reads_rig_checks() {
        let t = parse_task(
            &json!({ "status": "success", "output": { "riggable": true, "rig_type": "quadruped" }, "credits_consumed": 0.0 }),
        );
        assert_eq!(
            (t.riggable, t.rig_type.as_deref()),
            (Some(true), Some("quadruped"))
        );
    }

    #[test]
    fn reads_tasks() {
        let t = parse_task(&json!({
            "status": "success", "progress": 100, "credits_consumed": 30.0,
            "output": { "model_url": "https://cdn.tripo3d.ai/x/model.glb?sig=1", "rendered_image_url": "https://cdn/p.webp" }
        }));
        assert!(t.is_finished());
        assert_eq!(t.credits, Some(30.0));
        assert_eq!(model_extension(t.model_url.as_deref().unwrap()), "glb");
        assert_eq!(model_extension("https://cdn/x/model.fbx"), "fbx");
        assert_eq!(model_extension("https://cdn/x/model"), "glb");

        let failed = parse_task(
            &json!({ "status": "failed", "progress": 10, "error_code": 2003, "error_message": "Bad image" }),
        );
        assert!(failed.is_finished());
        assert_eq!(failed.error.as_deref(), Some("Bad image (code 2003)"));
        assert!(!parse_task(&json!({ "status": "queued" })).is_finished());
    }

    #[test]
    fn error_messages_include_the_suggestion() {
        let v = json!({ "code": 2010, "message": "You don't have enough credit", "suggestion": "Please purchase more credit" });
        assert_eq!(
            error_message(&v),
            "You don't have enough credit. Please purchase more credit"
        );
    }
}
