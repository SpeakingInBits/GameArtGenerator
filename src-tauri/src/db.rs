//! The generation log in `history.db` (SQLite): one row per image generation, 3D model job and audio generation,
//! successful or not. Images, models and audio are files on disk; the database stores their paths.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: i32 = 4;

#[derive(Debug, Clone)]
pub struct GenerationRow {
    pub id: String,
    pub created_at: String,
    pub status: Status,
    pub model: String,
    pub prompt: String,
    pub n: u32,
    pub aspect_ratio: String,
    pub resolution: String,
    pub quality: String,
    /// Stored paths (see `AppPaths::to_stored`).
    pub reference_images: Vec<String>,
    pub images: Vec<String>,
    pub cost_usd: f64,
    pub cost_source: CostSource,
    pub estimated_cost_usd: f64,
    pub duration_s: f64,
    pub error: Option<String>,
    pub usage: Option<String>,
}

/// One image sent to Tripo: the view it shows (`image` for a single picture, or `front`, `left`, `back`, `right`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInput {
    pub view: String,
    /// Stored path (see `AppPaths::to_stored`).
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct ModelJobRow {
    pub id: String,
    pub created_at: String,
    /// `model` (image to 3D model), `rig` (add a skeleton) or `animate` (apply animations to a rigged model).
    pub kind: String,
    /// The job this one builds on: a rig job's model job, or an animate job's rig job.
    pub parent_id: Option<String>,
    pub status: Status,
    /// Stored path of what the job was started from: the image for a model, the model file for rig and animate.
    pub source: String,
    pub inputs: Vec<ModelInput>,
    /// `tripo::ModelOptions` as JSON.
    pub options: String,
    pub task_id: Option<String>,
    /// Stored path of the job's folder.
    pub folder: String,
    pub model_path: Option<String>,
    pub preview_path: Option<String>,
    pub credits: Option<f64>,
    pub cost_usd: f64,
    pub estimated_cost_usd: f64,
    pub duration_s: f64,
    pub error: Option<String>,
}

/// One audio generation: a prompt sent to ElevenLabs, the app's audio server or ComfyUI, with its clips.
#[derive(Debug, Clone)]
pub struct AudioJobRow {
    pub id: String,
    pub created_at: String,
    pub status: Status,
    /// `elevenlabs`, `audioServer` or `comfyui`.
    pub provider: String,
    /// Where it ran: "ElevenLabs" or the server's name.
    pub source_name: String,
    /// Model id, or the ComfyUI workflow's name.
    pub model: String,
    /// `sfx` or `music`.
    pub kind: String,
    pub prompt: String,
    /// The request's settings as JSON (length, variations, loop, seed…).
    pub params: String,
    /// Stored paths of the saved clips.
    pub clips: Vec<String>,
    /// ElevenLabs credits used, when known.
    pub credits: Option<f64>,
    pub cost_usd: f64,
    pub duration_s: f64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    Error,
    /// A 3D model job that Tripo is still working on.
    Running,
}

/// Where `cost_usd` came from: the API's reported cost, the estimate (if the API didn't report one), or nothing
/// (failed generations aren't charged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CostSource {
    Api,
    Estimate,
    None,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
            Status::Running => "running",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "ok" => Status::Ok,
            "running" => Status::Running,
            _ => Status::Error,
        }
    }
}

impl CostSource {
    fn as_str(self) -> &'static str {
        match self {
            CostSource::Api => "api",
            CostSource::Estimate => "estimate",
            CostSource::None => "none",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "api" => CostSource::Api,
            "estimate" => CostSource::Estimate,
            _ => CostSource::None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub generations: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub images: u64,
    /// 3D models made (successful jobs).
    pub models: u64,
    /// Audio clips made.
    pub audio_clips: u64,
    /// Images and 3D models together.
    pub cost_usd: f64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path)
            .map_err(|e| format!("Couldn't open the history database: {e}"))?;
        let db = Self { conn };
        db.migrate()
            .map_err(|e| format!("Couldn't set up the history database: {e}"))?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Self {
        let db = Self {
            conn: Connection::open_in_memory().unwrap(),
        };
        db.migrate().unwrap();
        db
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i32 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE generations (
                    id                 TEXT PRIMARY KEY,
                    created_at         TEXT NOT NULL,   -- local time, ISO 8601
                    status             TEXT NOT NULL,   -- ok | error
                    model              TEXT NOT NULL,
                    prompt             TEXT NOT NULL,
                    n                  INTEGER NOT NULL,
                    aspect_ratio       TEXT NOT NULL,
                    resolution         TEXT NOT NULL,
                    quality            TEXT NOT NULL,
                    reference_images   TEXT NOT NULL,   -- JSON array of paths
                    images             TEXT NOT NULL,   -- JSON array of paths
                    cost_usd           REAL NOT NULL,
                    cost_source        TEXT NOT NULL,   -- api | estimate | none
                    estimated_cost_usd REAL NOT NULL,
                    duration_s         REAL NOT NULL,
                    error              TEXT,
                    usage              TEXT             -- the API's usage object, as JSON
                );
                CREATE INDEX generations_created_at ON generations (created_at);",
            )?;
        }
        if version < 2 {
            self.conn.execute_batch(
                "CREATE TABLE model_jobs (
                    id                 TEXT PRIMARY KEY,
                    created_at         TEXT NOT NULL,   -- local time, ISO 8601
                    status             TEXT NOT NULL,   -- running | ok | error
                    source             TEXT NOT NULL,   -- the image the user picked
                    inputs             TEXT NOT NULL,   -- JSON array of {view, path} sent to Tripo
                    options            TEXT NOT NULL,   -- JSON ModelOptions
                    task_id            TEXT,
                    folder             TEXT NOT NULL,
                    model_path         TEXT,
                    preview_path       TEXT,
                    credits            REAL,
                    cost_usd           REAL NOT NULL,
                    estimated_cost_usd REAL NOT NULL,
                    duration_s         REAL NOT NULL,
                    error              TEXT
                );
                CREATE INDEX model_jobs_created_at ON model_jobs (created_at);",
            )?;
        }
        if version < 3 {
            self.conn.execute_batch(
                "ALTER TABLE model_jobs ADD COLUMN kind TEXT NOT NULL DEFAULT 'model';  -- model | rig | animate
                 ALTER TABLE model_jobs ADD COLUMN parent_id TEXT;",
            )?;
        }
        if version < 4 {
            self.conn.execute_batch(
                "CREATE TABLE audio_jobs (
                    id          TEXT PRIMARY KEY,
                    created_at  TEXT NOT NULL,   -- local time, ISO 8601
                    status      TEXT NOT NULL,   -- ok | error
                    provider    TEXT NOT NULL,   -- elevenlabs | audioServer | comfyui
                    source_name TEXT NOT NULL,
                    model       TEXT NOT NULL,
                    kind        TEXT NOT NULL,   -- sfx | music
                    prompt      TEXT NOT NULL,
                    params      TEXT NOT NULL,   -- JSON
                    clips       TEXT NOT NULL,   -- JSON array of paths
                    credits     REAL,
                    cost_usd    REAL NOT NULL,
                    duration_s  REAL NOT NULL,
                    error       TEXT
                );
                CREATE INDEX audio_jobs_created_at ON audio_jobs (created_at);",
            )?;
        }
        if version < SCHEMA_VERSION {
            self.conn
                .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(())
    }

    pub fn insert(&self, g: &GenerationRow) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO generations (id, created_at, status, model, prompt, n, aspect_ratio, resolution, quality,
                    reference_images, images, cost_usd, cost_source, estimated_cost_usd, duration_s, error, usage)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    g.id,
                    g.created_at,
                    g.status.as_str(),
                    g.model,
                    g.prompt,
                    g.n,
                    g.aspect_ratio,
                    g.resolution,
                    g.quality,
                    serde_json::to_string(&g.reference_images).unwrap(),
                    serde_json::to_string(&g.images).unwrap(),
                    g.cost_usd,
                    g.cost_source.as_str(),
                    g.estimated_cost_usd,
                    g.duration_s,
                    g.error,
                    g.usage,
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("Couldn't save the generation to the history: {e}"))
    }

    /// All generations, newest first.
    pub fn all(&self) -> Result<Vec<GenerationRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, created_at, status, model, prompt, n, aspect_ratio, resolution, quality,
                        reference_images, images, cost_usd, cost_source, estimated_cost_usd, duration_s, error, usage
                 FROM generations ORDER BY created_at DESC, rowid DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let refs: String = r.get(9)?;
                let images: String = r.get(10)?;
                Ok(GenerationRow {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    status: Status::parse(&r.get::<_, String>(2)?),
                    model: r.get(3)?,
                    prompt: r.get(4)?,
                    n: r.get(5)?,
                    aspect_ratio: r.get(6)?,
                    resolution: r.get(7)?,
                    quality: r.get(8)?,
                    reference_images: serde_json::from_str(&refs).unwrap_or_default(),
                    images: serde_json::from_str(&images).unwrap_or_default(),
                    cost_usd: r.get(11)?,
                    cost_source: CostSource::parse(&r.get::<_, String>(12)?),
                    estimated_cost_usd: r.get(13)?,
                    duration_s: r.get(14)?,
                    error: r.get(15)?,
                    usage: r.get(16)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn totals(&self) -> Result<Totals, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(status = 'ok'), 0),
                        COALESCE(SUM(json_array_length(images)), 0),
                        COALESCE(SUM(cost_usd), 0),
                        (SELECT COUNT(*) FROM model_jobs WHERE status = 'ok' AND kind = 'model'),
                        (SELECT COALESCE(SUM(cost_usd), 0) FROM model_jobs),
                        (SELECT COALESCE(SUM(json_array_length(clips)), 0) FROM audio_jobs),
                        (SELECT COALESCE(SUM(cost_usd), 0) FROM audio_jobs)
                 FROM generations",
                [],
                |r| {
                    let generations: u64 = r.get(0)?;
                    let succeeded: u64 = r.get(1)?;
                    let image_cost: f64 = r.get(3)?;
                    let model_cost: f64 = r.get(5)?;
                    let audio_cost: f64 = r.get(7)?;
                    Ok(Totals {
                        generations,
                        succeeded,
                        failed: generations - succeeded,
                        images: r.get(2)?,
                        models: r.get(4)?,
                        audio_clips: r.get(6)?,
                        cost_usd: image_cost + model_cost + audio_cost,
                    })
                },
            )
            .map_err(|e| e.to_string())
    }

    pub fn save_model_job(&self, j: &ModelJobRow) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO model_jobs (id, created_at, status, source, inputs, options, task_id, folder,
                    model_path, preview_path, credits, cost_usd, estimated_cost_usd, duration_s, error, kind, parent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    j.id,
                    j.created_at,
                    j.status.as_str(),
                    j.source,
                    serde_json::to_string(&j.inputs).unwrap(),
                    j.options,
                    j.task_id,
                    j.folder,
                    j.model_path,
                    j.preview_path,
                    j.credits,
                    j.cost_usd,
                    j.estimated_cost_usd,
                    j.duration_s,
                    j.error,
                    j.kind,
                    j.parent_id,
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("Couldn't save the 3D model job to the history: {e}"))
    }

    /// All 3D model jobs, newest first.
    pub fn model_jobs(&self) -> Result<Vec<ModelJobRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, created_at, status, source, inputs, options, task_id, folder, model_path, preview_path,
                        credits, cost_usd, estimated_cost_usd, duration_s, error, kind, parent_id
                 FROM model_jobs ORDER BY created_at DESC, rowid DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let inputs: String = r.get(4)?;
                Ok(ModelJobRow {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    status: Status::parse(&r.get::<_, String>(2)?),
                    source: r.get(3)?,
                    inputs: serde_json::from_str(&inputs).unwrap_or_default(),
                    options: r.get(5)?,
                    task_id: r.get(6)?,
                    folder: r.get(7)?,
                    model_path: r.get(8)?,
                    preview_path: r.get(9)?,
                    credits: r.get(10)?,
                    cost_usd: r.get(11)?,
                    estimated_cost_usd: r.get(12)?,
                    duration_s: r.get(13)?,
                    error: r.get(14)?,
                    kind: r.get(15)?,
                    parent_id: r.get(16)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn save_audio_job(&self, j: &AudioJobRow) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO audio_jobs (id, created_at, status, provider, source_name, model, kind, prompt,
                    params, clips, credits, cost_usd, duration_s, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    j.id,
                    j.created_at,
                    j.status.as_str(),
                    j.provider,
                    j.source_name,
                    j.model,
                    j.kind,
                    j.prompt,
                    j.params,
                    serde_json::to_string(&j.clips).unwrap(),
                    j.credits,
                    j.cost_usd,
                    j.duration_s,
                    j.error,
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("Couldn't save the audio to the history: {e}"))
    }

    /// All audio generations, newest first.
    pub fn audio_jobs(&self) -> Result<Vec<AudioJobRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, created_at, status, provider, source_name, model, kind, prompt, params, clips, credits,
                        cost_usd, duration_s, error
                 FROM audio_jobs ORDER BY created_at DESC, rowid DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let clips: String = r.get(9)?;
                Ok(AudioJobRow {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    status: Status::parse(&r.get::<_, String>(2)?),
                    provider: r.get(3)?,
                    source_name: r.get(4)?,
                    model: r.get(5)?,
                    kind: r.get(6)?,
                    prompt: r.get(7)?,
                    params: r.get(8)?,
                    clips: serde_json::from_str(&clips).unwrap_or_default(),
                    credits: r.get(10)?,
                    cost_usd: r.get(11)?,
                    duration_s: r.get(12)?,
                    error: r.get(13)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Whether a generation, 3D model job or audio job already has this id.
    pub fn exists(&self, id: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM generations WHERE id = ?1 UNION ALL SELECT 1 FROM model_jobs WHERE id = ?1
                 UNION ALL SELECT 1 FROM audio_jobs WHERE id = ?1",
                [id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, status: Status, cost: f64, images: usize) -> GenerationRow {
        GenerationRow {
            id: id.into(),
            created_at: format!("2026-11-14T10:00:0{}", id.len() % 10),
            status,
            model: "grok-imagine-image".into(),
            prompt: "a sword".into(),
            n: images as u32,
            aspect_ratio: "1:1".into(),
            resolution: "1k".into(),
            quality: "standard".into(),
            reference_images: vec!["images/a/ref_1.png".into()],
            images: (0..images)
                .map(|i| format!("images/a/image_{i}.jpg"))
                .collect(),
            cost_usd: cost,
            cost_source: if status == Status::Ok {
                CostSource::Api
            } else {
                CostSource::None
            },
            estimated_cost_usd: cost,
            duration_s: 3.2,
            error: (status == Status::Error).then(|| "boom".into()),
            usage: None,
        }
    }

    #[test]
    fn records_and_totals() {
        let db = Database::open_in_memory();
        db.insert(&sample("a", Status::Ok, 0.022, 1)).unwrap();
        db.insert(&sample("bb", Status::Ok, 0.08, 2)).unwrap();
        db.insert(&sample("ccc", Status::Error, 0.0, 0)).unwrap();
        let t = db.totals().unwrap();
        assert_eq!(
            (t.generations, t.succeeded, t.failed, t.images),
            (3, 2, 1, 3)
        );
        assert!((t.cost_usd - 0.102).abs() < 1e-9);
        let all = db.all().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "ccc"); // newest first
        assert_eq!(all[0].error.as_deref(), Some("boom"));
        assert_eq!(all[2].reference_images, ["images/a/ref_1.png"]);
        assert!(db.exists("bb") && !db.exists("zz"));
    }

    #[test]
    fn model_jobs_count_toward_totals() {
        let db = Database::open_in_memory();
        db.insert(&sample("a", Status::Ok, 0.04, 1)).unwrap();
        let mut job = ModelJobRow {
            id: "m".into(),
            created_at: "2026-11-15T10:00:00".into(),
            kind: "model".into(),
            parent_id: None,
            status: Status::Running,
            source: "images/a/image_1.jpg".into(),
            inputs: vec![ModelInput {
                view: "front".into(),
                path: "models/m/view_front.png".into(),
            }],
            options: "{}".into(),
            task_id: Some("t1".into()),
            folder: "models/m".into(),
            model_path: None,
            preview_path: None,
            credits: None,
            cost_usd: 0.0,
            estimated_cost_usd: 0.3,
            duration_s: 0.0,
            error: None,
        };
        db.save_model_job(&job).unwrap();
        assert!(db.exists("m"));
        assert_eq!(db.totals().unwrap().models, 0);

        job.status = Status::Ok;
        job.credits = Some(30.0);
        job.cost_usd = 0.3;
        job.model_path = Some("models/m/model.glb".into());
        db.save_model_job(&job).unwrap();
        let t = db.totals().unwrap();
        assert_eq!((t.generations, t.models), (1, 1));
        assert!((t.cost_usd - 0.34).abs() < 1e-9);
        let jobs = db.model_jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, Status::Ok);
        assert_eq!(jobs[0].inputs[0].view, "front");

        // Rigging adds to the cost but isn't another model.
        let rig = ModelJobRow {
            id: "r".into(),
            kind: "rig".into(),
            parent_id: Some("m".into()),
            cost_usd: 0.25,
            ..job.clone()
        };
        db.save_model_job(&rig).unwrap();
        let t = db.totals().unwrap();
        assert_eq!(t.models, 1);
        assert!((t.cost_usd - 0.59).abs() < 1e-9);
        let rig = db
            .model_jobs()
            .unwrap()
            .into_iter()
            .find(|j| j.id == "r")
            .unwrap();
        assert_eq!(
            (rig.kind.as_str(), rig.parent_id.as_deref()),
            ("rig", Some("m"))
        );
    }

    #[test]
    fn audio_jobs_count_toward_totals() {
        let db = Database::open_in_memory();
        let job = AudioJobRow {
            id: "a1".into(),
            created_at: "2026-11-15T13:00:00".into(),
            status: Status::Ok,
            provider: "elevenlabs".into(),
            source_name: "ElevenLabs".into(),
            model: "eleven_text_to_sound_v2".into(),
            kind: "sfx".into(),
            prompt: "sword clash".into(),
            params: "{}".into(),
            clips: vec!["audio/a1/clip_1.wav".into(), "audio/a1/clip_2.wav".into()],
            credits: Some(400.0),
            cost_usd: 0.088,
            duration_s: 3.0,
            error: None,
        };
        db.save_audio_job(&job).unwrap();
        assert!(db.exists("a1"));
        let t = db.totals().unwrap();
        assert_eq!(t.audio_clips, 2);
        assert!((t.cost_usd - 0.088).abs() < 1e-9);
        let jobs = db.audio_jobs().unwrap();
        assert_eq!(jobs[0].clips.len(), 2);
        assert_eq!(jobs[0].credits, Some(400.0));
    }

    #[test]
    fn empty_totals() {
        let t = Database::open_in_memory().totals().unwrap();
        assert_eq!(t.generations, 0);
        assert_eq!(t.cost_usd, 0.0);
    }
}
