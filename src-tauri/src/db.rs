//! The generation log: one row per generation, successful or not, in `history.db` (SQLite).
//! Images are files on disk; the database stores their paths.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

const SCHEMA_VERSION: i32 = 1;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    Error,
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
        }
    }
    fn parse(s: &str) -> Self {
        if s == "ok" { Status::Ok } else { Status::Error }
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
                        COALESCE(SUM(cost_usd), 0)
                 FROM generations",
                [],
                |r| {
                    let generations: u64 = r.get(0)?;
                    let succeeded: u64 = r.get(1)?;
                    Ok(Totals {
                        generations,
                        succeeded,
                        failed: generations - succeeded,
                        images: r.get(2)?,
                        cost_usd: r.get(3)?,
                    })
                },
            )
            .map_err(|e| e.to_string())
    }

    pub fn exists(&self, id: &str) -> bool {
        self.conn
            .query_row("SELECT 1 FROM generations WHERE id = ?1", [id], |_| Ok(()))
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
    fn empty_totals() {
        let t = Database::open_in_memory().totals().unwrap();
        assert_eq!(t.generations, 0);
        assert_eq!(t.cost_usd, 0.0);
    }
}
