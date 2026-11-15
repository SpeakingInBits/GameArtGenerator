//! Game Art Generator: Tauri backend. The UI (in `../src`) calls the commands below.

mod db;
mod grok;
mod paths;
mod secrets;
mod settings;
mod updates;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager, State, ipc::InvokeBody};

use db::{CostSource, Database, GenerationRow, Status, Totals};
use grok::{GenerateParams, GrokClient, ModelInfo};
use paths::{AppPaths, InstallMode};
use secrets::KeySource;
use settings::Settings;
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

    fn reload_client(&self) {
        *self.client.lock().unwrap() = secrets::api_key().0.map(|k| Arc::new(GrokClient::new(k)));
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

#[derive(Serialize)]
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
        api_key_source: secrets::api_key().1,
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
    settings: Settings,
) -> Result<(), String> {
    if let Some(dir) = settings
        .images_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("Can't use {dir} for images: {e}"))?;
        let _ = app.asset_protocol_scope().allow_directory(dir, true);
    }
    settings.save(&state.paths.settings_file())?;
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
fn set_api_key(state: State<'_, AppState>, key: String) -> Result<KeySource, String> {
    secrets::set_api_key(&key)?;
    state.reload_client();
    Ok(secrets::api_key().1)
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

    let id = loop {
        let id = format!(
            "{}-{:04x}",
            started.format("%Y%m%d-%H%M%S"),
            rand::random::<u16>()
        );
        if !state.db.lock().unwrap().exists(&id) {
            break id;
        }
    };
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

    {
        let mut s = state.session.lock().unwrap();
        s.last_cost = Some(row.cost_usd);
        s.cost += row.cost_usd;
        s.count += 1;
    }
    Ok(view(&state, &app, row))
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
            let _ = std::fs::create_dir_all(&images);
            let scope = app.asset_protocol_scope();
            let _ = scope.allow_directory(&state.paths.data_dir, true);
            let _ = scope.allow_directory(&images, true);
            app.manage(state);
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
