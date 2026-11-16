//! Game Art Generator: Tauri backend. The UI (in `../src`) calls the commands below.

mod audio;
mod audio_commands;
mod audio_post;
mod db;
mod grok;
mod paths;
mod secrets;
mod settings;
mod sheet;
mod tripo;
mod updates;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, ipc::InvokeBody};

use db::{CostSource, Database, GenerationRow, ModelInput, ModelJobRow, Status, Totals};
use grok::{GenerateParams, GrokClient, ModelInfo};
use paths::{AppPaths, InstallMode};
use secrets::{KeySource, Service};
use settings::Settings;
use tripo::{ModelOptions, RigOptions, TaskInput, TripoClient};
use updates::{UpdateState, UpdateStatus};

struct Session {
    last_cost: Option<f64>,
    cost: f64,
    count: u32,
}

struct AppState {
    paths: AppPaths,
    db: Mutex<Database>,
    settings: Mutex<Settings>,
    models: Mutex<Vec<ModelInfo>>,
    client: Mutex<Option<Arc<GrokClient>>>,
    tripo: Mutex<Option<Arc<TripoClient>>>,
    elevenlabs: Mutex<Option<Arc<audio::ElevenLabsClient>>>,
    /// Held during an ElevenLabs generation, so the credits it used can be read off the account.
    elevenlabs_turn: tokio::sync::Mutex<()>,
    /// Progress (0–100) of 3D model jobs that are still running, by job id.
    progress: Mutex<HashMap<String, u32>>,
    session: Mutex<Session>,
}

impl AppState {
    fn client(&self) -> Result<Arc<GrokClient>, String> {
        self.client
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "No xAI API key is set. Add one in Settings.".to_string())
    }

    fn tripo(&self) -> Result<Arc<TripoClient>, String> {
        self.tripo
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "No Tripo API key is set. Add one in Settings.".to_string())
    }

    fn reload_client(&self) {
        *self.client.lock().unwrap() = secrets::api_key(Service::Xai)
            .0
            .map(|k| Arc::new(GrokClient::new(k)));
        *self.tripo.lock().unwrap() = secrets::api_key(Service::Tripo)
            .0
            .map(|k| Arc::new(TripoClient::new(k)));
        *self.elevenlabs.lock().unwrap() = secrets::api_key(Service::ElevenLabs)
            .0
            .map(|k| Arc::new(audio::ElevenLabsClient::new(k)));
    }

    fn elevenlabs(&self) -> Result<Arc<audio::ElevenLabsClient>, String> {
        self.elevenlabs
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "No ElevenLabs API key is set. Add one in Settings.".to_string())
    }

    fn audio_root(&self) -> PathBuf {
        match &self.settings.lock().unwrap().audio_dir {
            Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => self.paths.data_dir.join("audio"),
        }
    }

    fn add_to_session(&self, cost: f64) {
        let mut s = self.session.lock().unwrap();
        s.last_cost = Some(cost);
        s.cost += cost;
        s.count += 1;
    }

    /// A new id for a generation or 3D model job, e.g. `20261115-104512-3fa2`.
    fn new_id(&self, when: &chrono::DateTime<chrono::Local>) -> String {
        loop {
            let id = format!(
                "{}-{:04x}",
                when.format("%Y%m%d-%H%M%S"),
                rand::random::<u16>()
            );
            if !self.db.lock().unwrap().exists(&id) {
                return id;
            }
        }
    }

    fn model(&self, id: &str) -> Result<ModelInfo, String> {
        self.models
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("Unknown model {id}."))
    }

    fn images_root(&self) -> PathBuf {
        match &self.settings.lock().unwrap().images_dir {
            Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => self.paths.default_images_dir(),
        }
    }
}

// ---- views sent to the UI -------------------------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageFile {
    path: String,
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationView {
    id: String,
    created_at: String,
    status: Status,
    model: String,
    prompt: String,
    n: u32,
    aspect_ratio: String,
    resolution: String,
    quality: String,
    reference_images: Vec<ImageFile>,
    images: Vec<ImageFile>,
    cost_usd: f64,
    cost_source: CostSource,
    estimated_cost_usd: f64,
    duration_s: f64,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Costs {
    last_usd: Option<f64>,
    session_usd: f64,
    session_count: u32,
    all_time: Totals,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    mode: InstallMode,
    data_dir: String,
    images_dir: String,
    default_images_dir: String,
    api_key_source: KeySource,
    tripo_key_source: KeySource,
    elevenlabs_key_source: KeySource,
    audio_dir: String,
    default_audio_dir: String,
    /// Tripo's preset animations for each body type it can rig.
    rig_animations: Vec<(&'static str, &'static [&'static str])>,
    max_animations: usize,
    max_reference_images: usize,
    aspect_ratios: Vec<&'static str>,
    repo_url: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelList {
    models: Vec<ModelInfo>,
    /// False when the built-in list is used because xAI couldn't be reached.
    live: bool,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Estimate {
    total_usd: f64,
    per_image_usd: f64,
    references_usd: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceInfo {
    path: String,
    name: String,
    width: u32,
    height: u32,
}

/// Lets the UI show images from these folders through the asset protocol.
fn allow_dirs<'a>(app: &AppHandle, dirs: impl IntoIterator<Item = &'a Path>) {
    let scope = app.asset_protocol_scope();
    let mut seen = HashSet::new();
    for dir in dirs {
        if seen.insert(dir.to_path_buf()) {
            let _ = scope.allow_directory(dir, false);
        }
    }
}

fn view(state: &AppState, app: &AppHandle, row: GenerationRow) -> GenerationView {
    let to_files = |stored: &[String]| -> Vec<ImageFile> {
        stored
            .iter()
            .map(|s| {
                let p = state.paths.resolve_stored(s);
                ImageFile {
                    exists: p.exists(),
                    path: p.to_string_lossy().into_owned(),
                }
            })
            .collect()
    };
    let reference_images = to_files(&row.reference_images);
    let images = to_files(&row.images);
    let parents: Vec<PathBuf> = reference_images
        .iter()
        .chain(&images)
        .filter(|f| f.exists)
        .filter_map(|f| Path::new(&f.path).parent().map(Path::to_path_buf))
        .collect();
    allow_dirs(app, parents.iter().map(PathBuf::as_path));
    GenerationView {
        id: row.id,
        created_at: row.created_at,
        status: row.status,
        model: row.model,
        prompt: row.prompt,
        n: row.n,
        aspect_ratio: row.aspect_ratio,
        resolution: row.resolution,
        quality: row.quality,
        reference_images,
        images,
        cost_usd: row.cost_usd,
        cost_source: row.cost_source,
        estimated_cost_usd: row.estimated_cost_usd,
        duration_s: row.duration_s,
        error: row.error,
    }
}

fn extension_for(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "jpg",
    }
}

// ---- commands -------------------------------------------------------------------------------------------------------

#[tauri::command]
fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: updates::current_version(),
        mode: state.paths.mode,
        data_dir: state.paths.data_dir.to_string_lossy().into_owned(),
        images_dir: state.images_root().to_string_lossy().into_owned(),
        default_images_dir: state
            .paths
            .default_images_dir()
            .to_string_lossy()
            .into_owned(),
        api_key_source: secrets::api_key(Service::Xai).1,
        tripo_key_source: secrets::api_key(Service::Tripo).1,
        elevenlabs_key_source: secrets::api_key(Service::ElevenLabs).1,
        audio_dir: state.audio_root().to_string_lossy().into_owned(),
        default_audio_dir: state
            .paths
            .data_dir
            .join("audio")
            .to_string_lossy()
            .into_owned(),
        rig_animations: tripo::RIG_TYPES
            .iter()
            .map(|t| (*t, tripo::animations_for(t)))
            .collect(),
        max_animations: tripo::MAX_ANIMATIONS,
        max_reference_images: grok::MAX_REFERENCE_IMAGES,
        aspect_ratios: grok::ASPECT_RATIOS.to_vec(),
        repo_url: updates::REPO_URL,
    }
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
) -> Result<(), String> {
    for (dir, what) in [
        (&settings.images_dir, "images"),
        (&settings.audio_dir, "audio"),
    ] {
        if let Some(dir) = dir.as_deref().filter(|d| !d.trim().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("Can't use {dir} for {what}: {e}"))?;
            let _ = app.asset_protocol_scope().allow_directory(dir, true);
        }
    }
    // Servers are managed by their own commands; the UI's copy of the settings may be older.
    settings.audio_servers = state.settings.lock().unwrap().audio_servers.clone();
    settings.save(&state.paths.settings_file())?;
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
fn set_api_key(
    state: State<'_, AppState>,
    key: String,
    service: Option<Service>,
) -> Result<KeySource, String> {
    let service = service.unwrap_or_default();
    secrets::set_api_key(service, &key)?;
    state.reload_client();
    Ok(secrets::api_key(service).1)
}

#[tauri::command]
async fn list_models(state: State<'_, AppState>) -> Result<ModelList, String> {
    let fetched = match state.client() {
        Ok(client) => client.list_models().await,
        Err(e) => Err(e),
    };
    let (models, live, error) = match fetched {
        Ok(models) if !models.is_empty() => (models, true, None),
        Ok(_) => (
            grok::fallback_models(),
            false,
            Some("xAI returned no image models.".to_string()),
        ),
        Err(e) => (grok::fallback_models(), false, Some(e)),
    };
    *state.models.lock().unwrap() = models.clone();
    Ok(ModelList {
        models,
        live,
        error,
    })
}

#[tauri::command]
fn estimate_cost(
    state: State<'_, AppState>,
    params: GenerateParams,
    references: usize,
) -> Result<Estimate, String> {
    let model = state.model(&params.model)?;
    let per_image_usd = model.output_price(&params.quality, &params.resolution, references > 0);
    let references_usd = references as f64 * model.input_image_price_usd;
    Ok(Estimate {
        total_usd: params.n as f64 * per_image_usd + references_usd,
        per_image_usd,
        references_usd,
    })
}

#[tauri::command]
fn add_reference(app: AppHandle, path: String) -> Result<ReferenceInfo, String> {
    let p = PathBuf::from(&path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let (width, height) = image::image_dimensions(&p)
        .map_err(|_| format!("{name} isn't an image this app can read."))?;
    let _ = app.asset_protocol_scope().allow_file(&p);
    Ok(ReferenceInfo {
        path,
        name,
        width,
        height,
    })
}

/// Saves an image pasted from the clipboard (sent as raw bytes) so it can be used as a reference.
#[tauri::command]
fn save_pasted_image(
    app: AppHandle,
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<ReferenceInfo, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("Expected image bytes.".into());
    };
    let format = image::guess_format(bytes)
        .map_err(|_| "The clipboard doesn't contain an image.".to_string())?;
    let ext = format.extensions_str().first().copied().unwrap_or("png");
    let dir = state.paths.pasted_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "pasted-{}.{ext}",
        chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")
    ));
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save the pasted image: {e}"))?;
    add_reference(app, path.to_string_lossy().into_owned())
}

/// The Paste button: saves the image on the clipboard (e.g. a screenshot) so it can be used as a reference.
#[tauri::command]
fn paste_reference(app: AppHandle, state: State<'_, AppState>) -> Result<ReferenceInfo, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    if let Ok(img) = clipboard.get_image() {
        let rgba =
            image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
                .ok_or("The clipboard image couldn't be read.")?;
        let dir = state.paths.pasted_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!(
            "pasted-{}.png",
            chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")
        ));
        rgba.save(&path)
            .map_err(|e| format!("Couldn't save the pasted image: {e}"))?;
        return add_reference(app, path.to_string_lossy().into_owned());
    }
    // Image files copied in Explorer.
    if let Ok(files) = clipboard.get().file_list()
        && let Some(first) = files.into_iter().next()
    {
        return add_reference(app, first.to_string_lossy().into_owned());
    }
    Err("The clipboard doesn't contain an image.".into())
}

#[tauri::command]
async fn generate(
    app: AppHandle,
    state: State<'_, AppState>,
    params: GenerateParams,
    references: Vec<String>,
) -> Result<GenerationView, String> {
    let prompt = params.prompt.trim();
    if prompt.is_empty() {
        return Err("Enter a prompt first.".into());
    }
    if !(1..=10).contains(&params.n) {
        return Err("The number of images must be between 1 and 10.".into());
    }
    let client = state.client()?;
    let model = state.model(&params.model)?;
    let params = GenerateParams {
        prompt: prompt.to_string(),
        ..params
    };
    let refs: Vec<PathBuf> = references.iter().map(PathBuf::from).collect();
    let estimate = model.estimate(params.n, &params.resolution, &params.quality, refs.len());
    let quality = if model.supports_quality {
        params.quality.clone()
    } else {
        "standard".to_string()
    };

    let started = chrono::Local::now();
    let t0 = Instant::now();
    let result = client.generate(&params, &model, &refs).await;
    let duration_s = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;

    let id = state.new_id(&started);
    let mut row = GenerationRow {
        id: id.clone(),
        created_at: started.format("%Y-%m-%dT%H:%M:%S").to_string(),
        status: Status::Error,
        model: params.model.clone(),
        prompt: params.prompt.clone(),
        n: params.n,
        aspect_ratio: params.aspect_ratio.clone(),
        resolution: params.resolution.clone(),
        quality,
        reference_images: references.clone(),
        images: vec![],
        cost_usd: 0.0,
        cost_source: CostSource::None,
        estimated_cost_usd: estimate,
        duration_s,
        error: None,
        usage: None,
    };

    let output = match result {
        Ok(output) => output,
        Err(e) => {
            row.error = Some(e.clone());
            state.db.lock().unwrap().insert(&row)?;
            return Err(e);
        }
    };

    // Save the images (and copies of the references, so the record is complete even if the originals move).
    let folder = state
        .images_root()
        .join(started.format("%Y-%m-%d").to_string())
        .join(&id);
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    row.reference_images = refs
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let ext = src
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_else(|| "png".into());
            let dst = folder.join(format!("ref_{}.{ext}", i + 1));
            match std::fs::copy(src, &dst) {
                Ok(_) => state.paths.to_stored(&dst),
                Err(_) => src.to_string_lossy().into_owned(),
            }
        })
        .collect();
    for (i, img) in output.images.iter().enumerate() {
        let dst = folder.join(format!("image_{}.{}", i + 1, extension_for(&img.mime_type)));
        std::fs::write(&dst, &img.bytes)
            .map_err(|e| format!("Couldn't save {}: {e}", dst.display()))?;
        row.images.push(state.paths.to_stored(&dst));
    }

    row.status = Status::Ok;
    row.cost_source = if output.cost_usd.is_some() {
        CostSource::Api
    } else {
        CostSource::Estimate
    };
    row.cost_usd = output.cost_usd.unwrap_or(estimate);
    row.usage = Some(output.usage.to_string());
    state.db.lock().unwrap().insert(&row)?;
    state.add_to_session(row.cost_usd);
    Ok(view(&state, &app, row))
}

// ---- 3D models (Tripo) ----------------------------------------------------------------------------------------------

/// Give up watching a Tripo task after this long; the job is marked failed with its task id.
const TRIPO_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const TRIPO_POLL: Duration = Duration::from_secs(3);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelInputView {
    view: String,
    path: String,
    exists: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelJobView {
    id: String,
    created_at: String,
    /// `model`, `rig` or `animate`.
    kind: String,
    parent_id: Option<String>,
    status: Status,
    progress: u32,
    source: ImageFile,
    inputs: Vec<ModelInputView>,
    options: serde_json::Value,
    task_id: Option<String>,
    folder: String,
    model: Option<ImageFile>,
    preview: Option<ImageFile>,
    credits: Option<f64>,
    cost_usd: f64,
    estimated_cost_usd: f64,
    duration_s: f64,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewCrop {
    path: String,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelEstimate {
    credits: f64,
    usd: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelInputArg {
    /// `image` (one picture) or `front`, `left`, `back`, `right`.
    view: String,
    path: String,
}

fn model_job_view(state: &AppState, app: &AppHandle, row: ModelJobRow) -> ModelJobView {
    let file = |stored: &str| {
        let p = state.paths.resolve_stored(stored);
        ImageFile {
            exists: p.exists(),
            path: p.to_string_lossy().into_owned(),
        }
    };
    let folder = state.paths.resolve_stored(&row.folder);
    if folder.exists() {
        allow_dirs(app, [folder.as_path()]);
    }
    let source = file(&row.source);
    if source.exists
        && let Some(parent) = Path::new(&source.path).parent()
    {
        allow_dirs(app, [parent]);
    }
    let progress = match row.status {
        Status::Running => state
            .progress
            .lock()
            .unwrap()
            .get(&row.id)
            .copied()
            .unwrap_or(0),
        _ => 100,
    };
    ModelJobView {
        progress,
        source,
        inputs: row
            .inputs
            .iter()
            .map(|i| {
                let f = file(&i.path);
                ModelInputView {
                    view: i.view.clone(),
                    path: f.path,
                    exists: f.exists,
                }
            })
            .collect(),
        options: serde_json::from_str(&row.options).unwrap_or_default(),
        folder: folder.to_string_lossy().into_owned(),
        model: row.model_path.as_deref().map(file),
        preview: row.preview_path.as_deref().map(file),
        id: row.id,
        created_at: row.created_at,
        kind: row.kind,
        parent_id: row.parent_id,
        status: row.status,
        task_id: row.task_id,
        credits: row.credits,
        cost_usd: row.cost_usd,
        estimated_cost_usd: row.estimated_cost_usd,
        duration_s: row.duration_s,
        error: row.error,
    }
}

fn find_model_job(state: &AppState, id: &str) -> Result<ModelJobRow, String> {
    state
        .db
        .lock()
        .unwrap()
        .model_jobs()?
        .into_iter()
        .find(|j| j.id == id)
        .ok_or_else(|| format!("No 3D model job {id}."))
}

/// Tells the UI a job changed.
fn emit_model_job(app: &AppHandle, row: ModelJobRow) {
    let state = app.state::<AppState>();
    let view = model_job_view(&state, app, row);
    let _ = app.emit("model-job", view);
}

fn seconds_since(created_at: &str) -> f64 {
    chrono::NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S")
        .map(|t| (chrono::Local::now().naive_local() - t).num_milliseconds() as f64 / 1000.0)
        .map(|s| (s.max(0.0) * 10.0).round() / 10.0)
        .unwrap_or(0.0)
}

/// Cuts a reference sheet into one square image per view, saved in the data folder's `views` folder.
#[tauri::command]
fn split_sheet(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<ViewCrop>, String> {
    let img = image::open(&path)
        .map_err(|e| format!("Couldn't read the image: {e}"))?
        .to_rgba8();
    let spans = sheet::find_views(&img);
    if spans.is_empty() {
        return Err("Couldn't find any views on a plain background in this image.".into());
    }
    let dir = state.paths.data_dir.join("views").join(format!(
        "{}-{:04x}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        rand::random::<u16>()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut crops = Vec::new();
    for (i, span) in spans.into_iter().enumerate() {
        let crop = sheet::crop_view(&img, span);
        let dst = dir.join(format!("view_{}.png", i + 1));
        crop.save(&dst)
            .map_err(|e| format!("Couldn't save a view: {e}"))?;
        crops.push(ViewCrop {
            path: dst.to_string_lossy().into_owned(),
            width: crop.width(),
            height: crop.height(),
        });
    }
    allow_dirs(&app, [dir.as_path()]);
    Ok(crops)
}

#[tauri::command]
fn estimate_model_cost(options: ModelOptions) -> ModelEstimate {
    let credits = options.estimate_credits();
    ModelEstimate {
        credits,
        usd: credits * tripo::USD_PER_CREDIT,
    }
}

/// Credits left on the Tripo account.
#[tauri::command]
async fn tripo_balance(state: State<'_, AppState>) -> Result<f64, String> {
    state.tripo()?.balance().await
}

#[tauri::command]
fn list_model_jobs(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<ModelJobView>, String> {
    let rows = state.db.lock().unwrap().model_jobs()?;
    Ok(rows
        .into_iter()
        .map(|r| model_job_view(&state, &app, r))
        .collect())
}

/// Starts a Tripo job and returns right away; `model-job` events report progress and the result.
#[tauri::command]
async fn make_3d_model(
    app: AppHandle,
    state: State<'_, AppState>,
    source: String,
    inputs: Vec<ModelInputArg>,
    options: ModelOptions,
) -> Result<ModelJobView, String> {
    let single = inputs.len() == 1 && inputs[0].view == "image";
    if !single {
        let mut seen = HashSet::new();
        for i in &inputs {
            if !tripo::VIEWS.contains(&i.view.as_str()) {
                return Err(format!("Unknown view {}.", i.view));
            }
            if !seen.insert(i.view.as_str()) {
                return Err(format!("Two images are set as the {} view.", i.view));
            }
        }
        if !seen.contains("front") {
            return Err("One of the images has to be the front view.".into());
        }
        if inputs.len() < 2 {
            return Err("Pick at least two views, or send the whole image instead.".into());
        }
    }
    let client = state.tripo()?;

    let started = chrono::Local::now();
    let id = state.new_id(&started);
    let folder = state
        .images_root()
        .join("models")
        .join(started.format("%Y-%m-%d").to_string())
        .join(&id);
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;

    // Keep copies of what was sent, so the record is complete even if the originals move.
    let mut copies = Vec::new();
    for i in &inputs {
        let src = PathBuf::from(&i.path);
        let ext = src
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "png".into());
        let name = if single {
            format!("input.{ext}")
        } else {
            format!("view_{}.{ext}", i.view)
        };
        let dst = folder.join(name);
        std::fs::copy(&src, &dst).map_err(|e| format!("Couldn't copy {}: {e}", src.display()))?;
        copies.push((i.view.clone(), dst));
    }

    let mut row = ModelJobRow {
        id: id.clone(),
        created_at: started.format("%Y-%m-%dT%H:%M:%S").to_string(),
        kind: "model".into(),
        parent_id: None,
        status: Status::Running,
        source: state.paths.to_stored(Path::new(&source)),
        inputs: copies
            .iter()
            .map(|(view, p)| ModelInput {
                view: view.clone(),
                path: state.paths.to_stored(p),
            })
            .collect(),
        options: serde_json::to_string(&options).unwrap(),
        task_id: None,
        folder: state.paths.to_stored(&folder),
        model_path: None,
        preview_path: None,
        credits: None,
        cost_usd: 0.0,
        estimated_cost_usd: options.estimate_credits() * tripo::USD_PER_CREDIT,
        duration_s: 0.0,
        error: None,
    };
    state.db.lock().unwrap().save_model_job(&row)?;

    let submitted = async {
        let mut tokens = Vec::new();
        for (view, path) in &copies {
            tokens.push((view.clone(), client.upload(path).await?));
        }
        let input = if single {
            TaskInput::Image(tokens.remove(0).1)
        } else {
            TaskInput::Views(tokens)
        };
        client.create_task(&input, &options).await
    }
    .await;

    match submitted {
        Ok(task_id) => {
            row.task_id = Some(task_id);
            state.db.lock().unwrap().save_model_job(&row)?;
            spawn_watch(app.clone(), id);
            Ok(model_job_view(&state, &app, row))
        }
        Err(e) => {
            row.status = Status::Error;
            row.error = Some(e.clone());
            row.duration_s = seconds_since(&row.created_at);
            state.db.lock().unwrap().save_model_job(&row)?;
            Err(e)
        }
    }
}

/// Polls a job's Tripo task until it finishes, then downloads the model and its preview render.
async fn watch_model_job(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let Ok(mut row) = find_model_job(&state, &id) else {
        return;
    };
    let Some(task_id) = row.task_id.clone() else {
        return;
    };
    let mut failures = 0;
    let task = loop {
        tokio::time::sleep(TRIPO_POLL).await;
        // Without a key the job stays running and is picked up again at the next start.
        let Ok(client) = state.tripo() else {
            return;
        };
        match client.task(&task_id).await {
            Ok(t) if t.is_finished() => break Ok((client, t)),
            Ok(t) => {
                failures = 0;
                state
                    .progress
                    .lock()
                    .unwrap()
                    .insert(id.clone(), t.progress);
                emit_model_job(&app, row.clone());
            }
            Err(e) => {
                failures += 1;
                if failures >= 20 {
                    // Leave it running: it's picked up again at the next start.
                    eprintln!("Stopped watching Tripo task {task_id}: {e}");
                    return;
                }
            }
        }
        if seconds_since(&row.created_at) > TRIPO_TIMEOUT.as_secs_f64() {
            break Err(format!(
                "Tripo didn't finish within {} minutes (task {task_id}).",
                TRIPO_TIMEOUT.as_secs() / 60
            ));
        }
    };
    state.progress.lock().unwrap().remove(&id);

    match task {
        Err(e) => {
            row.status = Status::Error;
            row.error = Some(e);
        }
        Ok((client, t)) => {
            row.credits = t.credits;
            row.cost_usd = t.credits.unwrap_or(0.0) * tripo::USD_PER_CREDIT;
            if t.status != "success" {
                row.status = Status::Error;
                row.error = Some(
                    t.error
                        .clone()
                        .unwrap_or_else(|| format!("The Tripo task ended as {}.", t.status)),
                );
            } else {
                let folder = state.paths.resolve_stored(&row.folder);
                let saved = if row.kind == "model" {
                    save_model_outputs(&client, &t, &folder).await
                } else {
                    save_rig_output(&client, &t, &folder, &row)
                        .await
                        .map(|p| (p, None))
                };
                match saved {
                    Ok((model, preview)) => {
                        row.status = Status::Ok;
                        row.model_path = Some(state.paths.to_stored(&model));
                        row.preview_path = preview.map(|p| state.paths.to_stored(&p));
                    }
                    Err(e) => {
                        row.status = Status::Error;
                        row.error = Some(format!(
                            "Tripo finished, but the file couldn't be saved: {e} (task {task_id})"
                        ));
                    }
                }
            }
        }
    }
    row.duration_s = seconds_since(&row.created_at);
    if let Err(e) = state.db.lock().unwrap().save_model_job(&row) {
        eprintln!("{e}");
    }
    if row.cost_usd > 0.0 {
        state.add_to_session(row.cost_usd);
    }
    // A finished rig goes straight on to its animations, if any were picked.
    if row.kind == "rig" && row.status == Status::Ok {
        spawn_animate(app.clone(), row.clone());
    }
    emit_model_job(&app, row);
}

/// Watches a job in the background. (A plain function, so the watcher can start the next job without its future
/// type containing itself.)
fn spawn_watch(app: AppHandle, id: String) {
    tauri::async_runtime::spawn(watch_model_job(app, id));
}

fn spawn_animate(app: AppHandle, rig: ModelJobRow) {
    let Ok(options) = serde_json::from_str::<RigOptions>(&rig.options) else {
        return;
    };
    if options.animations.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = start_animate_job(&app, &rig, &options).await {
            eprintln!("Couldn't start the animations: {e}");
        }
    });
}

/// Where a rigged or animated model goes in its model's folder: `rigged.glb`, `animated_idle_walk.fbx`, … with a
/// number added if that name is taken.
fn rig_output_path(folder: &Path, row: &ModelJobRow, ext: &str) -> PathBuf {
    let stem = if row.kind == "animate" {
        let names: Vec<String> = serde_json::from_str::<RigOptions>(&row.options)
            .map(|o| o.animations)
            .unwrap_or_default()
            .iter()
            .map(|a| a.trim_start_matches("preset:").replace(':', "_"))
            .collect();
        format!("animated_{}", names.join("_"))
    } else {
        "rigged".to_string()
    };
    let mut path = folder.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    path
}

async fn save_rig_output(
    client: &TripoClient,
    task: &tripo::TaskStatus,
    folder: &Path,
    row: &ModelJobRow,
) -> Result<PathBuf, String> {
    let url = task
        .model_url
        .as_deref()
        .ok_or("Tripo didn't return a model.")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let path = rig_output_path(folder, row, &tripo::model_extension(url));
    std::fs::write(&path, client.download(url).await?).map_err(|e| e.to_string())?;
    Ok(path)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RigCheck {
    riggable: bool,
    rig_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RigEstimate {
    rig_credits: f64,
    animation_credits: f64,
    usd: f64,
}

/// A finished model job, which rigging builds on.
fn riggable_model_job(state: &AppState, job_id: &str) -> Result<(ModelJobRow, String), String> {
    let job = find_model_job(state, job_id)?;
    if job.kind != "model" || job.status != Status::Ok {
        return Err("Only finished 3D models can be rigged.".into());
    }
    let task_id = job
        .task_id
        .clone()
        .ok_or("This model has no Tripo task to rig from.")?;
    Ok((job, task_id))
}

/// Asks Tripo whether a model can be rigged and what body type it looks like. Free; takes a few seconds.
#[tauri::command]
async fn check_rig(state: State<'_, AppState>, job_id: String) -> Result<RigCheck, String> {
    let (_, model_task) = riggable_model_job(&state, &job_id)?;
    let client = state.tripo()?;
    let task_id = client.create_rig_check(&model_task).await?;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let t = client.task(&task_id).await?;
        if t.is_finished() {
            if t.status != "success" {
                return Err(t
                    .error
                    .unwrap_or_else(|| format!("The rig check ended as {}.", t.status)));
            }
            return Ok(RigCheck {
                riggable: t.riggable.unwrap_or(false),
                rig_type: t.rig_type,
            });
        }
    }
    Err("The rig check is taking too long. Pick the body type yourself.".into())
}

#[tauri::command]
fn estimate_rig_cost(options: RigOptions) -> RigEstimate {
    let rig_credits = RigOptions::rig_credits();
    let animation_credits = options.animation_credits();
    RigEstimate {
        rig_credits,
        animation_credits,
        usd: (rig_credits + animation_credits) * tripo::USD_PER_CREDIT,
    }
}

/// Rigs a finished model, then (if any are picked) applies animations. Returns the rig job right away;
/// `model-job` events report both jobs.
#[tauri::command]
async fn rig_model(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
    options: RigOptions,
) -> Result<ModelJobView, String> {
    options.validate()?;
    let (parent, model_task) = riggable_model_job(&state, &job_id)?;
    let client = state.tripo()?;
    let started = chrono::Local::now();
    let mut row = ModelJobRow {
        id: state.new_id(&started),
        created_at: started.format("%Y-%m-%dT%H:%M:%S").to_string(),
        kind: "rig".into(),
        parent_id: Some(parent.id.clone()),
        status: Status::Running,
        source: parent.model_path.clone().unwrap_or_default(),
        inputs: vec![],
        options: serde_json::to_string(&options).unwrap(),
        task_id: None,
        folder: parent.folder.clone(),
        model_path: None,
        preview_path: None,
        credits: None,
        cost_usd: 0.0,
        estimated_cost_usd: RigOptions::rig_credits() * tripo::USD_PER_CREDIT,
        duration_s: 0.0,
        error: None,
    };
    state.db.lock().unwrap().save_model_job(&row)?;
    match client.create_rig(&model_task, &options).await {
        Ok(task_id) => {
            row.task_id = Some(task_id);
            state.db.lock().unwrap().save_model_job(&row)?;
            spawn_watch(app.clone(), row.id.clone());
            Ok(model_job_view(&state, &app, row))
        }
        Err(e) => {
            row.status = Status::Error;
            row.error = Some(e.clone());
            state.db.lock().unwrap().save_model_job(&row)?;
            Err(e)
        }
    }
}

/// Applies the picked animations to a finished rig, as a job of its own.
async fn start_animate_job(
    app: &AppHandle,
    rig: &ModelJobRow,
    options: &RigOptions,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let rig_task = rig.task_id.clone().ok_or("The rig has no Tripo task.")?;
    let started = chrono::Local::now();
    let mut row = ModelJobRow {
        id: state.new_id(&started),
        created_at: started.format("%Y-%m-%dT%H:%M:%S").to_string(),
        kind: "animate".into(),
        parent_id: Some(rig.id.clone()),
        status: Status::Running,
        source: rig.model_path.clone().unwrap_or_default(),
        inputs: vec![],
        options: rig.options.clone(),
        task_id: None,
        folder: rig.folder.clone(),
        model_path: None,
        preview_path: None,
        credits: None,
        cost_usd: 0.0,
        estimated_cost_usd: options.animation_credits() * tripo::USD_PER_CREDIT,
        duration_s: 0.0,
        error: None,
    };
    state.db.lock().unwrap().save_model_job(&row)?;
    let created = match state.tripo() {
        Ok(client) => client.create_retarget(&rig_task, options).await,
        Err(e) => Err(e),
    };
    match created {
        Ok(task_id) => {
            row.task_id = Some(task_id);
            state.db.lock().unwrap().save_model_job(&row)?;
            emit_model_job(app, row.clone());
            spawn_watch(app.clone(), row.id);
            Ok(())
        }
        Err(e) => {
            row.status = Status::Error;
            row.error = Some(e.clone());
            state.db.lock().unwrap().save_model_job(&row)?;
            emit_model_job(app, row);
            Err(e)
        }
    }
}

async fn save_model_outputs(
    client: &TripoClient,
    task: &tripo::TaskStatus,
    folder: &Path,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let url = task
        .model_url
        .as_deref()
        .ok_or("Tripo didn't return a model.")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let model = folder.join(format!("model.{}", tripo::model_extension(url)));
    std::fs::write(&model, client.download(url).await?).map_err(|e| e.to_string())?;

    // The preview render is a nice-to-have; a failed download doesn't fail the job.
    let mut preview = None;
    if let Some(url) = task.preview_url.as_deref()
        && let Ok(bytes) = client.download(url).await
    {
        let ext = image::guess_format(&bytes)
            .ok()
            .and_then(|f| f.extensions_str().first().copied())
            .unwrap_or("png");
        let path = folder.join(format!("preview.{ext}"));
        if std::fs::write(&path, &bytes).is_ok() {
            preview = Some(path);
        }
    }
    Ok((model, preview))
}

/// At startup: keep watching jobs that were still running when the app closed (Tripo has already charged for them).
fn resume_model_jobs(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(jobs) = state.db.lock().unwrap().model_jobs() else {
        return;
    };
    for mut job in jobs.into_iter().filter(|j| j.status == Status::Running) {
        if job.task_id.is_some() {
            spawn_watch(app.clone(), job.id);
        } else {
            job.status = Status::Error;
            job.error = Some("The app closed before the job was sent to Tripo.".into());
            let _ = state.db.lock().unwrap().save_model_job(&job);
        }
    }
}

#[tauri::command]
fn list_history(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<GenerationView>, String> {
    let rows = state.db.lock().unwrap().all()?;
    Ok(rows.into_iter().map(|r| view(&state, &app, r)).collect())
}

#[tauri::command]
fn get_costs(state: State<'_, AppState>) -> Result<Costs, String> {
    let all_time = state.db.lock().unwrap().totals()?;
    let s = state.session.lock().unwrap();
    Ok(Costs {
        last_usd: s.last_cost,
        session_usd: s.cost,
        session_count: s.count,
        all_time,
    })
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn show_in_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{path}\""))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        open_path(
            Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(path),
        )
    }
}

#[tauri::command]
fn copy_image(path: String) -> Result<(), String> {
    let img = image::open(&path)
        .map_err(|e| format!("Couldn't read the image: {e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: img.into_raw().into(),
        })
        .map_err(|e| format!("Couldn't copy the image: {e}"))
}

#[tauri::command]
fn get_update_status(state: State<'_, UpdateState>) -> UpdateStatus {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
fn check_for_updates(app: AppHandle) {
    updates::check(app);
}

#[tauri::command]
fn restart_to_update() -> Result<(), String> {
    updates::restart_to_update()
}

// ---- startup --------------------------------------------------------------------------------------------------------

fn init_state() -> Result<AppState, String> {
    let paths = AppPaths::resolve()?;
    std::fs::create_dir_all(&paths.data_dir).map_err(|e| {
        format!(
            "Couldn't create the data folder {}: {e}",
            paths.data_dir.display()
        )
    })?;
    let db = Database::open(&paths.database())?;
    let settings = Settings::load(&paths.settings_file());
    let state = AppState {
        db: Mutex::new(db),
        settings: Mutex::new(settings),
        models: Mutex::new(grok::fallback_models()),
        client: Mutex::new(None),
        tripo: Mutex::new(None),
        elevenlabs: Mutex::new(None),
        elevenlabs_turn: tokio::sync::Mutex::new(()),
        progress: Mutex::new(HashMap::new()),
        session: Mutex::new(Session {
            last_cost: None,
            cost: 0.0,
            count: 0,
        }),
        paths,
    };
    state.reload_client();
    Ok(state)
}

pub fn run() {
    let state = match init_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(UpdateState(Mutex::new(updates::initial_status())))
        .setup(move |app| {
            let images = state.images_root();
            let audio = state.audio_root();
            let _ = std::fs::create_dir_all(&images);
            let _ = std::fs::create_dir_all(&audio);
            let scope = app.asset_protocol_scope();
            let _ = scope.allow_directory(&state.paths.data_dir, true);
            let _ = scope.allow_directory(&images, true);
            let _ = scope.allow_directory(&audio, true);
            app.manage(state);
            resume_model_jobs(app.handle());
            updates::check(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            get_settings,
            save_settings,
            set_api_key,
            list_models,
            estimate_cost,
            add_reference,
            save_pasted_image,
            paste_reference,
            generate,
            split_sheet,
            estimate_model_cost,
            tripo_balance,
            list_model_jobs,
            make_3d_model,
            check_rig,
            estimate_rig_cost,
            rig_model,
            audio_commands::list_audio_sources,
            audio_commands::save_audio_server,
            audio_commands::remove_audio_server,
            audio_commands::elevenlabs_usage,
            audio_commands::inspect_comfy_workflow,
            audio_commands::list_comfy_workflows,
            audio_commands::save_comfy_workflow,
            audio_commands::remove_comfy_workflow,
            audio_commands::generate_audio,
            audio_commands::list_audio_jobs,
            list_history,
            get_costs,
            open_path,
            show_in_folder,
            copy_image,
            get_update_status,
            check_for_updates,
            restart_to_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Game Art Generator");
}
