//! Commands for the Audio tab: generating sound effects and music with ElevenLabs, the app's own audio server or
//! ComfyUI; managing self-hosted servers and ComfyUI workflows; and the audio log.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::audio::{
    self, AudioServerClient, Clip, ComfyClient, ComfyMapping, ComfyWorkflow, FieldCandidate,
    ServerModel,
};
use crate::audio_post;
use crate::db::{AudioJobRow, Status};
use crate::settings::AudioServer;
use crate::{AppState, allow_dirs, secrets};

const MAX_VARIATIONS: u32 = 4;
const COMFY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

// ---- sources ------------------------------------------------------------------------------------------------------

/// Something the Audio tab can generate with: an ElevenLabs product, a model on an audio server, or a ComfyUI
/// workflow on a ComfyUI server.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSource {
    /// `elevenlabs:sfx`, `elevenlabs:music`, `server:<server id>:<model id>` or `comfy:<server id>:<workflow id>`.
    pub id: String,
    /// `elevenlabs`, `audioServer` or `comfyui`.
    pub provider: String,
    /// Where it runs: "ElevenLabs" or the server's name.
    pub group: String,
    pub label: String,
    /// `sfx` or `music`.
    pub kind: String,
    pub min_seconds: f64,
    pub max_seconds: f64,
    /// ElevenLabs model choices (newest last).
    pub models: Vec<String>,
    /// Which controls apply.
    pub auto_length: bool,
    pub looping: bool,
    pub influence: bool,
    pub instrumental: bool,
    pub negative: bool,
    pub seed: bool,
    pub license: Option<String>,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub has_token: bool,
    pub ok: bool,
    /// The server's description (models, GPU), or why it couldn't be reached.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSources {
    pub sources: Vec<AudioSource>,
    pub servers: Vec<ServerStatus>,
    pub elevenlabs_key: bool,
}

fn elevenlabs_sources() -> Vec<AudioSource> {
    vec![
        AudioSource {
            id: "elevenlabs:sfx".into(),
            provider: "elevenlabs".into(),
            group: "ElevenLabs".into(),
            label: "Sound effects".into(),
            kind: "sfx".into(),
            min_seconds: 0.5,
            max_seconds: 30.0,
            models: audio::ELEVENLABS_SFX_MODELS
                .iter()
                .map(|m| m.to_string())
                .collect(),
            auto_length: true,
            looping: true,
            influence: true,
            instrumental: false,
            negative: false,
            seed: false,
            license: Some("Commercial use on paid plans".into()),
            device: None,
        },
        AudioSource {
            id: "elevenlabs:music".into(),
            provider: "elevenlabs".into(),
            group: "ElevenLabs".into(),
            label: "Music".into(),
            kind: "music".into(),
            min_seconds: 3.0,
            max_seconds: 600.0,
            models: audio::ELEVENLABS_MUSIC_MODELS
                .iter()
                .map(|m| m.to_string())
                .collect(),
            auto_length: true,
            looping: false,
            influence: false,
            instrumental: true,
            negative: false,
            seed: false,
            license: Some("Commercial use on paid plans".into()),
            device: None,
        },
    ]
}

fn server_source(server: &AudioServer, m: &ServerModel) -> AudioSource {
    AudioSource {
        id: format!("server:{}:{}", server.id, m.id),
        provider: "audioServer".into(),
        group: server.name.clone(),
        label: m.label.clone(),
        kind: m.kind.clone(),
        min_seconds: 1.0,
        max_seconds: m.max_seconds,
        models: vec![],
        auto_length: false,
        looping: false,
        influence: false,
        instrumental: false,
        negative: m.supports_negative,
        seed: true,
        license: m.license.clone(),
        device: m.device.clone(),
    }
}

fn comfy_source(server: &AudioServer, w: &ComfyWorkflow) -> AudioSource {
    AudioSource {
        id: format!("comfy:{}:{}", server.id, w.id),
        provider: "comfyui".into(),
        group: server.name.clone(),
        label: w.name.clone(),
        kind: w.kind.clone(),
        min_seconds: 1.0,
        max_seconds: w.max_seconds,
        models: vec![],
        auto_length: w.mapping.seconds.is_empty(),
        looping: false,
        influence: false,
        instrumental: false,
        negative: !w.mapping.negative.is_empty(),
        seed: !w.mapping.seed.is_empty(),
        license: None,
        device: None,
    }
}

fn servers(state: &AppState) -> Vec<AudioServer> {
    state.settings.lock().unwrap().audio_servers.clone()
}

fn find_server(state: &AppState, id: &str) -> Result<AudioServer, String> {
    servers(state)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "That server isn't in Settings any more.".to_string())
}

/// What one server offers, and whether it's reachable.
async fn probe_server(
    server: AudioServer,
    workflows: Vec<ComfyWorkflow>,
) -> (ServerStatus, Vec<AudioSource>) {
    let token = secrets::server_token(&server.id);
    let mut status = ServerStatus {
        id: server.id.clone(),
        name: server.name.clone(),
        kind: server.kind.clone(),
        url: server.url.clone(),
        has_token: token.is_some(),
        ok: false,
        detail: String::new(),
    };
    let mut sources = Vec::new();
    if server.kind == "comfyui" {
        match ComfyClient::new(&server.url, token).describe().await {
            Ok(device) => {
                status.ok = true;
                status.detail = format!("ComfyUI on {device} · {} workflows", workflows.len());
                sources = workflows.iter().map(|w| comfy_source(&server, w)).collect();
            }
            Err(e) => status.detail = e,
        }
    } else {
        match AudioServerClient::new(&server.url, token).info().await {
            Ok(info) => {
                status.ok = true;
                let names: Vec<String> = info.models.iter().map(|m| m.label.clone()).collect();
                status.detail = format!("{} · {}", info.name, names.join(", "));
                sources = info
                    .models
                    .iter()
                    .map(|m| server_source(&server, m))
                    .collect();
            }
            Err(e) => status.detail = e,
        }
    }
    (status, sources)
}

#[tauri::command]
pub async fn list_audio_sources(state: State<'_, AppState>) -> Result<AudioSources, String> {
    let workflows = load_workflows(&state);
    let handles: Vec<_> = servers(&state)
        .into_iter()
        .map(|s| tauri::async_runtime::spawn(probe_server(s, workflows.clone())))
        .collect();
    let mut sources = elevenlabs_sources();
    let mut statuses = Vec::new();
    for h in handles {
        if let Ok((status, mut found)) = h.await {
            statuses.push(status);
            sources.append(&mut found);
        }
    }
    Ok(AudioSources {
        sources,
        servers: statuses,
        elevenlabs_key: secrets::api_key(secrets::Service::ElevenLabs).0.is_some(),
    })
}

// ---- servers ------------------------------------------------------------------------------------------------------

/// Adds or updates a server. A `token` of `Some("")` removes the saved token; `None` keeps it.
#[tauri::command]
pub fn save_audio_server(
    state: State<'_, AppState>,
    mut server: AudioServer,
    token: Option<String>,
) -> Result<AudioServer, String> {
    server.name = server.name.trim().to_string();
    server.url = server.url.trim().trim_end_matches('/').to_string();
    if server.name.is_empty() {
        return Err("Give the server a name.".into());
    }
    if !(server.url.starts_with("http://") || server.url.starts_with("https://")) {
        return Err("The address should start with http:// or https://".into());
    }
    if !["audioServer", "comfyui"].contains(&server.kind.as_str()) {
        return Err(format!("Unknown server type {}.", server.kind));
    }
    if server.id.is_empty() {
        server.id = format!("{:08x}", rand::random::<u32>());
    }
    if let Some(t) = token {
        secrets::set_server_token(&server.id, &t)?;
    }
    let mut settings = state.settings.lock().unwrap().clone();
    match settings
        .audio_servers
        .iter_mut()
        .find(|s| s.id == server.id)
    {
        Some(existing) => *existing = server.clone(),
        None => settings.audio_servers.push(server.clone()),
    }
    settings.save(&state.paths.settings_file())?;
    *state.settings.lock().unwrap() = settings;
    Ok(server)
}

#[tauri::command]
pub fn remove_audio_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap().clone();
    settings.audio_servers.retain(|s| s.id != id);
    settings.save(&state.paths.settings_file())?;
    *state.settings.lock().unwrap() = settings;
    let _ = secrets::set_server_token(&id, "");
    Ok(())
}

#[tauri::command]
pub async fn elevenlabs_usage(state: State<'_, AppState>) -> Result<audio::Subscription, String> {
    state.elevenlabs()?.subscription().await
}

// ---- ComfyUI workflows ----------------------------------------------------------------------------------------------

fn workflows_dir(state: &AppState) -> PathBuf {
    state.paths.data_dir.join("comfy-workflows")
}

fn load_workflows(state: &AppState) -> Vec<ComfyWorkflow> {
    let mut out: Vec<ComfyWorkflow> = std::fs::read_dir(workflows_dir(state))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDraft {
    pub name: String,
    pub workflow: Value,
    pub mapping: ComfyMapping,
    pub candidates: Vec<FieldCandidate>,
}

/// Reads an exported workflow and guesses which inputs the app should fill in.
#[tauri::command]
pub fn inspect_comfy_workflow(path: String) -> Result<WorkflowDraft, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    let workflow: Value =
        serde_json::from_str(&text).map_err(|_| "This file isn't JSON.".to_string())?;
    audio::validate_workflow(&workflow)?;
    let name = Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['_', '-'], " "))
        .unwrap_or_else(|| "Workflow".into());
    Ok(WorkflowDraft {
        name,
        mapping: audio::detect_mapping(&workflow),
        candidates: audio::candidates(&workflow),
        workflow,
    })
}

#[tauri::command]
pub fn list_comfy_workflows(state: State<'_, AppState>) -> Vec<ComfyWorkflow> {
    load_workflows(&state)
}

#[tauri::command]
pub fn save_comfy_workflow(
    state: State<'_, AppState>,
    mut workflow: ComfyWorkflow,
) -> Result<ComfyWorkflow, String> {
    audio::validate_workflow(&workflow.workflow)?;
    workflow.name = workflow.name.trim().to_string();
    if workflow.name.is_empty() {
        return Err("Give the workflow a name.".into());
    }
    if workflow.mapping.prompt.is_empty() {
        return Err("Pick which input gets the prompt.".into());
    }
    if !["sfx", "music"].contains(&workflow.kind.as_str()) {
        return Err("Pick sound effects or music.".into());
    }
    if workflow.id.is_empty() {
        workflow.id = format!("{:08x}", rand::random::<u32>());
    }
    let dir = workflows_dir(&state);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&workflow).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", workflow.id)), json)
        .map_err(|e| format!("Couldn't save the workflow: {e}"))?;
    Ok(workflow)
}

#[tauri::command]
pub fn remove_comfy_workflow(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Unknown workflow.".into());
    }
    let path = workflows_dir(&state).join(format!("{id}.json"));
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ---- generating ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioRequest {
    /// An `AudioSource` id.
    pub source: String,
    pub prompt: String,
    #[serde(default)]
    pub negative: Option<String>,
    /// `None` lets the provider pick (where it can).
    #[serde(default)]
    pub seconds: Option<f64>,
    pub variations: u32,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub looping: bool,
    #[serde(default)]
    pub influence: Option<f64>,
    #[serde(default)]
    pub instrumental: bool,
    /// ElevenLabs model id.
    #[serde(default)]
    pub model: Option<String>,
    /// Trim silence, set the peak to -1 dBFS and save WAV.
    pub clean: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClipView {
    pub path: String,
    pub exists: bool,
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioJobView {
    pub id: String,
    pub created_at: String,
    pub status: Status,
    pub provider: String,
    pub source_name: String,
    pub model: String,
    pub kind: String,
    pub prompt: String,
    pub params: Value,
    pub clips: Vec<AudioClipView>,
    pub credits: Option<f64>,
    pub cost_usd: f64,
    pub duration_s: f64,
    pub error: Option<String>,
}

fn job_view(state: &AppState, app: &AppHandle, row: AudioJobRow) -> AudioJobView {
    let params: Value = serde_json::from_str(&row.params).unwrap_or_default();
    let seconds = params["clipSeconds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let clips: Vec<AudioClipView> = row
        .clips
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let p = state.paths.resolve_stored(s);
            AudioClipView {
                exists: p.exists(),
                path: p.to_string_lossy().into_owned(),
                seconds: seconds.get(i).and_then(Value::as_f64),
            }
        })
        .collect();
    let dirs: Vec<PathBuf> = clips
        .iter()
        .filter(|c| c.exists)
        .filter_map(|c| Path::new(&c.path).parent().map(Path::to_path_buf))
        .collect();
    allow_dirs(app, dirs.iter().map(PathBuf::as_path));
    AudioJobView {
        id: row.id,
        created_at: row.created_at,
        status: row.status,
        provider: row.provider,
        source_name: row.source_name,
        model: row.model,
        kind: row.kind,
        prompt: row.prompt,
        params,
        clips,
        credits: row.credits,
        cost_usd: row.cost_usd,
        duration_s: row.duration_s,
        error: row.error,
    }
}

#[tauri::command]
pub fn list_audio_jobs(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AudioJobView>, String> {
    let rows = state.db.lock().unwrap().audio_jobs()?;
    Ok(rows
        .into_iter()
        .map(|r| job_view(&state, &app, r))
        .collect())
}

/// A file-name-friendly version of the prompt's first words: "Heavy sword hits a shield!" -> "heavy_sword_hits_a".
fn slug(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(4)
        .map(|w| w.to_lowercase())
        .collect();
    let s = words.join("_");
    if s.is_empty() {
        "audio".into()
    } else {
        s.chars().take(40).collect()
    }
}

/// What ran, and how: the provider's name, model and kind for the log.
struct Ran {
    provider: &'static str,
    source_name: String,
    model: String,
    kind: String,
    clips: Vec<Clip>,
    credits: Option<f64>,
    seed: Option<u64>,
}

async fn run_elevenlabs(state: &AppState, req: &AudioRequest, music: bool) -> Result<Ran, String> {
    let client = state.elevenlabs()?;
    let models = if music {
        &audio::ELEVENLABS_MUSIC_MODELS[..]
    } else {
        &audio::ELEVENLABS_SFX_MODELS[..]
    };
    let model = req
        .model
        .clone()
        .filter(|m| models.contains(&m.as_str()))
        .unwrap_or_else(|| models[0].to_string());
    // One ElevenLabs request at a time, so the credits used can be read from the account before and after.
    let _turn = state.elevenlabs_turn.lock().await;
    let before = client.subscription().await.ok();
    let mut clips = Vec::new();
    for _ in 0..req.variations {
        let clip = if music {
            client
                .music(&req.prompt, req.seconds, req.instrumental, &model)
                .await
        } else {
            client
                .sound_effect(&req.prompt, req.seconds, req.looping, req.influence, &model)
                .await
        };
        match clip {
            Ok(c) => clips.push(c),
            // Keep what was already made (and paid for); report the error only if nothing was.
            Err(e) if clips.is_empty() => return Err(e),
            Err(_) => break,
        }
    }
    let after = client.subscription().await.ok();
    let credits = match (before, after) {
        (Some(b), Some(a)) if a.used >= b.used => Some(a.used - b.used),
        _ => None,
    };
    Ok(Ran {
        provider: "elevenlabs",
        source_name: "ElevenLabs".into(),
        model,
        kind: if music { "music" } else { "sfx" }.into(),
        clips,
        credits,
        seed: None,
    })
}

async fn run_server(
    state: &AppState,
    req: &AudioRequest,
    server_id: &str,
    model: &str,
) -> Result<Ran, String> {
    let server = find_server(state, server_id)?;
    let client = AudioServerClient::new(&server.url, secrets::server_token(&server.id));
    let info = client.info().await?;
    let m = info
        .models
        .iter()
        .find(|m| m.id == model)
        .ok_or_else(|| format!("{} no longer offers {model}.", server.name))?;
    let seconds = req.seconds.unwrap_or(5.0).clamp(1.0, m.max_seconds);
    let (clips, seed) = client
        .generate(
            model,
            &req.prompt,
            req.negative.as_deref().filter(|n| !n.trim().is_empty()),
            seconds,
            req.seed,
            req.variations,
        )
        .await?;
    Ok(Ran {
        provider: "audioServer",
        source_name: server.name,
        model: m.label.clone(),
        kind: m.kind.clone(),
        clips,
        credits: None,
        seed,
    })
}

async fn run_comfy(
    state: &AppState,
    req: &AudioRequest,
    server_id: &str,
    workflow_id: &str,
) -> Result<Ran, String> {
    let server = find_server(state, server_id)?;
    let workflow = load_workflows(state)
        .into_iter()
        .find(|w| w.id == workflow_id)
        .ok_or("That workflow was removed.")?;
    let client = ComfyClient::new(&server.url, secrets::server_token(&server.id));
    let first_seed = req.seed.unwrap_or_else(|| rand::random::<u32>() as u64);
    let seconds = req.seconds.map(|s| s.clamp(1.0, workflow.max_seconds));
    let mut clips = Vec::new();
    for i in 0..req.variations {
        let filled = audio::fill_workflow(
            &workflow.workflow,
            &workflow.mapping,
            &req.prompt,
            req.negative.as_deref(),
            seconds,
            first_seed + i as u64,
        );
        match client.run(&filled, COMFY_TIMEOUT).await {
            Ok(mut c) => clips.append(&mut c),
            Err(e) if clips.is_empty() => return Err(e),
            Err(_) => break,
        }
    }
    Ok(Ran {
        provider: "comfyui",
        source_name: server.name,
        model: workflow.name,
        kind: workflow.kind,
        clips,
        credits: None,
        seed: Some(first_seed),
    })
}

/// Saves one clip, cleaned up as WAV when asked (and when it can be decoded). Returns the path and its length.
fn save_clip(
    folder: &Path,
    stem: &str,
    clip: &Clip,
    clean: bool,
    looping: bool,
) -> Result<(PathBuf, Option<f64>), String> {
    if clean && let Ok(mut d) = audio_post::decode(&clip.bytes, &clip.ext) {
        // A loop's start and end must stay as made, or it stops being seamless.
        if !looping {
            d.trim_silence();
        }
        d.normalize();
        let path = folder.join(format!("{stem}.wav"));
        std::fs::write(&path, d.to_wav()?)
            .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
        return Ok((path, Some(d.duration())));
    }
    let path = folder.join(format!("{stem}.{}", clip.ext));
    std::fs::write(&path, &clip.bytes)
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    let seconds = audio_post::decode(&clip.bytes, &clip.ext)
        .ok()
        .map(|d| d.duration());
    Ok((path, seconds))
}

#[tauri::command]
pub async fn generate_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    request: AudioRequest,
) -> Result<AudioJobView, String> {
    let mut req = request;
    req.prompt = req.prompt.trim().to_string();
    if req.prompt.is_empty() {
        return Err("Describe the sound first.".into());
    }
    req.variations = req.variations.clamp(1, MAX_VARIATIONS);

    let started = chrono::Local::now();
    let t0 = Instant::now();
    let parts: Vec<&str> = req.source.splitn(3, ':').collect();
    let result = match parts.as_slice() {
        ["elevenlabs", "sfx"] => run_elevenlabs(&state, &req, false).await,
        ["elevenlabs", "music"] => run_elevenlabs(&state, &req, true).await,
        ["server", server, model] => run_server(&state, &req, server, model).await,
        ["comfy", server, workflow] => run_comfy(&state, &req, server, workflow).await,
        _ => Err(format!("Unknown audio source {}.", req.source)),
    };

    let id = state.new_id(&started);
    let mut params = serde_json::to_value(&req).unwrap_or(json!({}));
    let mut row = AudioJobRow {
        id: id.clone(),
        created_at: started.format("%Y-%m-%dT%H:%M:%S").to_string(),
        status: Status::Error,
        provider: parts
            .first()
            .map(|p| match *p {
                "server" => "audioServer",
                "comfy" => "comfyui",
                other => other,
            })
            .unwrap_or("")
            .to_string(),
        source_name: String::new(),
        model: String::new(),
        kind: if req.source == "elevenlabs:music" {
            "music"
        } else {
            "sfx"
        }
        .into(),
        prompt: req.prompt.clone(),
        params: params.to_string(),
        clips: vec![],
        credits: None,
        cost_usd: 0.0,
        duration_s: 0.0,
        error: None,
    };

    let ran = match result {
        Ok(r) => r,
        Err(e) => {
            row.duration_s = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
            row.error = Some(e.clone());
            state.db.lock().unwrap().save_audio_job(&row)?;
            return Err(e);
        }
    };

    let folder = state
        .audio_root()
        .join(started.format("%Y-%m-%d").to_string())
        .join(&id);
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    let stem = slug(&req.prompt);
    let mut seconds = Vec::new();
    for (i, clip) in ran.clips.iter().enumerate() {
        let name = if ran.clips.len() == 1 {
            stem.clone()
        } else {
            format!("{stem}_{}", i + 1)
        };
        let (path, len) = save_clip(&folder, &name, clip, req.clean, req.looping)?;
        row.clips.push(state.paths.to_stored(&path));
        seconds.push(len);
    }
    params["clipSeconds"] = json!(seconds);
    if let Some(seed) = ran.seed {
        params["seedUsed"] = json!(seed);
    }
    row.params = params.to_string();
    row.status = Status::Ok;
    row.provider = ran.provider.into();
    row.source_name = ran.source_name;
    row.model = ran.model;
    row.kind = ran.kind;
    row.credits = ran.credits;
    if let Some(c) = ran.credits {
        let rate = state.settings.lock().unwrap().elevenlabs_usd_per_1k_credits;
        row.cost_usd = c / 1000.0 * rate;
    }
    row.duration_s = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    state.db.lock().unwrap().save_audio_job(&row)?;
    if row.cost_usd > 0.0 {
        state.add_to_session(row.cost_usd);
    }
    allow_dirs(&app, [folder.as_path()]);
    Ok(job_view(&state, &app, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_make_file_names() {
        assert_eq!(slug("Heavy sword hits a shield!"), "heavy_sword_hits_a");
        assert_eq!(slug("¡¿"), "audio");
        assert_eq!(
            slug("Goblin-death squeal, short"),
            "goblin_death_squeal_short"
        );
    }
}
