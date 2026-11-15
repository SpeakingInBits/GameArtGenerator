//! Client for the xAI Grok Imagine image API.
//!
//! * `GET  /v1/image-generation-models`: models and their prices
//! * `POST /v1/images/generations`: text to image
//! * `POST /v1/images/edits`: reference images + text to image
//!
//! Each image response reports what it cost in `usage.cost_in_usd_ticks` (10,000,000,000 ticks = $1). That's the
//! cost the app records; the estimate is only shown before generating.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use image::{ImageFormat, ImageReader, codecs::jpeg::JpegEncoder, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const API_BASE: &str = "https://api.x.ai/v1";
const TICKS_PER_USD: f64 = 10_000_000_000.0;
pub const MAX_REFERENCE_IMAGES: usize = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Reference images with a longer side than this are downscaled before upload.
const MAX_REFERENCE_SIDE: u32 = 2048;
const MAX_RAW_UPLOAD_BYTES: u64 = 4 * 1024 * 1024;

pub const ASPECT_RATIOS: &[&str] = &[
    "auto", "1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "2:1", "1:2", "19.5:9", "9:19.5",
    "20:9", "9:20", "21:9", "5:2",
];

/// What each reference image adds to the bill, measured from real responses; xAI doesn't publish it.
/// Only used for the estimate.
fn input_image_price(model_id: &str) -> f64 {
    match model_id {
        "grok-imagine-image" => 0.002,
        "grok-imagine-image-2.0" => 0.01,
        _ => 0.0,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceTier {
    pub quality: String,
    pub resolution: String,
    pub price_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub image_price_usd: f64,
    /// Price per output image by quality and resolution. Empty for models with a single price.
    pub pricing: Vec<PriceTier>,
    pub max_prompt_length: u32,
    pub input_image_price_usd: f64,
    pub supports_quality: bool,
    pub resolutions: Vec<String>,
}

impl ModelInfo {
    fn new(
        id: &str,
        image_price_usd: f64,
        pricing: Vec<PriceTier>,
        max_prompt_length: u32,
    ) -> Self {
        let mut resolutions: Vec<String> = if pricing.is_empty() {
            vec!["1k".into(), "2k".into()]
        } else {
            let mut r: Vec<String> = pricing.iter().map(|p| p.resolution.clone()).collect();
            r.sort();
            r.dedup();
            r
        };
        resolutions.sort_by(|a, b| res_value(a).total_cmp(&res_value(b)));
        Self {
            id: id.to_string(),
            image_price_usd,
            supports_quality: !pricing.is_empty(),
            pricing,
            max_prompt_length,
            input_image_price_usd: input_image_price(id),
            resolutions,
        }
    }

    /// Price of one output image. `auto` quality is low for new images and medium with reference images.
    pub fn output_price(&self, quality: &str, resolution: &str, has_refs: bool) -> f64 {
        if self.pricing.is_empty() {
            return self.image_price_usd;
        }
        let quality = match quality {
            "auto" if has_refs => "medium",
            "auto" => "low",
            q => q,
        };
        self.pricing
            .iter()
            .find(|p| p.quality == quality && p.resolution == resolution)
            .map(|p| p.price_usd)
            .unwrap_or(self.image_price_usd)
    }

    pub fn estimate(&self, n: u32, resolution: &str, quality: &str, refs: usize) -> f64 {
        n as f64 * self.output_price(quality, resolution, refs > 0)
            + refs as f64 * self.input_image_price_usd
    }
}

fn res_value(r: &str) -> f64 {
    r.trim_end_matches('k').parse().unwrap_or(0.0)
}

/// Used until (or if) the live list can't be fetched.
pub fn fallback_models() -> Vec<ModelInfo> {
    let tier = |q: &str, r: &str, p: f64| PriceTier {
        quality: q.into(),
        resolution: r.into(),
        price_usd: p,
    };
    vec![
        ModelInfo::new(
            "grok-imagine-image-2.0",
            0.06,
            vec![
                tier("low", "1k", 0.04),
                tier("low", "1.5k", 0.05),
                tier("low", "2k", 0.06),
                tier("medium", "1k", 0.06),
                tier("medium", "1.5k", 0.07),
                tier("medium", "2k", 0.08),
            ],
            64000,
        ),
        ModelInfo::new("grok-imagine-image", 0.02, vec![], 16000),
    ]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateParams {
    pub model: String,
    pub prompt: String,
    pub n: u32,
    pub aspect_ratio: String,
    pub resolution: String,
    pub quality: String,
}

pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

pub struct GenerateOutput {
    pub images: Vec<GeneratedImage>,
    pub cost_usd: Option<f64>,
    pub usage: Value,
}

pub struct GrokClient {
    http: reqwest::Client,
    api_key: String,
}

impl GrokClient {
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
            .map_err(|e| format!("Couldn't reach the xAI API: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Couldn't read the xAI API's response: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "xAI API error {}: {}",
                status.as_u16(),
                error_message(&text)
            ));
        }
        serde_json::from_str(&text)
            .map_err(|_| "The xAI API returned a response that isn't JSON.".to_string())
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        let data = self
            .send(self.http.get(format!("{API_BASE}/image-generation-models")))
            .await?;
        let mut models: Vec<ModelInfo> = data["models"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                let pricing = m["pricing"]
                    .as_array()
                    .map(|a| a.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| {
                        Some(PriceTier {
                            quality: p["quality"].as_str()?.to_string(),
                            resolution: p["resolution"].as_str()?.to_string(),
                            price_usd: p["price_per_image"].as_f64()? / TICKS_PER_USD,
                        })
                    })
                    .collect();
                Some(ModelInfo::new(
                    id,
                    m["image_price"].as_f64().unwrap_or(0.0) / TICKS_PER_USD,
                    pricing,
                    m["max_prompt_length"].as_u64().unwrap_or(16000) as u32,
                ))
            })
            .collect();
        // Newest first.
        models.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(models)
    }

    pub async fn generate(
        &self,
        params: &GenerateParams,
        model: &ModelInfo,
        references: &[std::path::PathBuf],
    ) -> Result<GenerateOutput, String> {
        if references.len() > MAX_REFERENCE_IMAGES {
            return Err(format!(
                "At most {MAX_REFERENCE_IMAGES} reference images can be sent."
            ));
        }
        let mut body = json!({
            "model": params.model,
            "prompt": params.prompt,
            "n": params.n,
            "response_format": "b64_json",
            "resolution": params.resolution,
        });
        if params.aspect_ratio != "auto" {
            body["aspect_ratio"] = json!(params.aspect_ratio);
        }
        if model.supports_quality {
            body["quality"] = json!(params.quality);
        }

        let endpoint = if references.is_empty() {
            "generations"
        } else {
            let mut images = Vec::with_capacity(references.len());
            for path in references {
                images.push(json!({ "url": image_data_uri(path)?, "type": "image_url" }));
            }
            body["images"] = Value::Array(images);
            "edits"
        };

        let data = self
            .send(
                self.http
                    .post(format!("{API_BASE}/images/{endpoint}"))
                    .json(&body),
            )
            .await?;

        let mut images = Vec::new();
        for item in data["data"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
        {
            let mime_type = item["mime_type"]
                .as_str()
                .unwrap_or("image/jpeg")
                .to_string();
            if let Some(b64) = item["b64_json"].as_str() {
                let bytes = B64
                    .decode(b64)
                    .map_err(|e| format!("The API returned an unreadable image: {e}"))?;
                images.push(GeneratedImage { bytes, mime_type });
            } else if let Some(url) = item["url"].as_str() {
                let resp = self
                    .http
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| format!("Couldn't download an image: {e}"))?;
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| format!("Couldn't download an image: {e}"))?
                    .to_vec();
                images.push(GeneratedImage { bytes, mime_type });
            }
        }
        if images.is_empty() {
            return Err("The API returned no images (the prompt may have been moderated).".into());
        }

        let usage = data.get("usage").cloned().unwrap_or(Value::Null);
        let cost_usd = usage["cost_in_usd_ticks"]
            .as_f64()
            .map(|t| t / TICKS_PER_USD);
        Ok(GenerateOutput {
            images,
            cost_usd,
            usage,
        })
    }
}

fn error_message(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return body.chars().take(500).collect();
    };
    let msg = match &v["error"] {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("message").and_then(|m| m.as_str()).map(String::from),
        _ => None,
    };
    msg.or_else(|| v["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(500).collect())
}

/// Encodes a reference image as a data URI. JPEG and PNG files of a reasonable size are sent as they are; anything
/// else (WebP, GIF, BMP, very large files) is converted, and downscaled if its longer side is over 2048 px.
pub fn image_data_uri(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't read reference image {name}: {e}");

    let reader = ImageReader::open(path)
        .map_err(|e| fail(&e))?
        .with_guessed_format()
        .map_err(|e| fail(&e))?;
    let format = reader
        .format()
        .ok_or_else(|| fail(&"not a supported image type"))?;
    let size = std::fs::metadata(path).map_err(|e| fail(&e))?.len();
    let (w, h) = image::image_dimensions(path).map_err(|e| fail(&e))?;

    let raw_ok = matches!(format, ImageFormat::Jpeg | ImageFormat::Png)
        && w.max(h) <= MAX_REFERENCE_SIDE
        && size <= MAX_RAW_UPLOAD_BYTES;
    if raw_ok {
        let mime = if format == ImageFormat::Png {
            "image/png"
        } else {
            "image/jpeg"
        };
        let bytes = std::fs::read(path).map_err(|e| fail(&e))?;
        return Ok(format!("data:{mime};base64,{}", B64.encode(bytes)));
    }

    let mut img = reader.decode().map_err(|e| fail(&e))?;
    if img.width().max(img.height()) > MAX_REFERENCE_SIDE {
        img = img.resize(MAX_REFERENCE_SIDE, MAX_REFERENCE_SIDE, FilterType::Lanczos3);
    }
    let mut buf = Cursor::new(Vec::new());
    let mime = if img.color().has_alpha() {
        img.to_rgba8()
            .write_to(&mut buf, ImageFormat::Png)
            .map_err(|e| fail(&e))?;
        "image/png"
    } else {
        JpegEncoder::new_with_quality(&mut buf, 92)
            .encode_image(&img.to_rgb8())
            .map_err(|e| fail(&e))?;
        "image/jpeg"
    };
    Ok(format!(
        "data:{mime};base64,{}",
        B64.encode(buf.into_inner())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_matches_observed_costs() {
        let models = fallback_models();
        let v2 = &models[0];
        let v1 = &models[1];
        // Observed: 2.0, low, 1k, one reference image -> $0.05.
        assert!((v2.estimate(1, "1k", "low", 1) - 0.05).abs() < 1e-9);
        // Observed: grok-imagine-image, n=1, one reference image -> $0.022.
        assert!((v1.estimate(1, "1k", "standard", 1) - 0.022).abs() < 1e-9);
        // auto quality is medium when editing.
        assert!((v2.output_price("auto", "2k", true) - 0.08).abs() < 1e-9);
        assert!((v2.output_price("auto", "2k", false) - 0.06).abs() < 1e-9);
        assert_eq!(v2.resolutions, ["1k", "1.5k", "2k"]);
    }

    #[test]
    fn reads_both_error_shapes() {
        assert_eq!(
            error_message(r#"{"code":"x","error":"Bad key"}"#),
            "Bad key"
        );
        assert_eq!(error_message(r#"{"error":{"message":"Nope"}}"#), "Nope");
        assert_eq!(error_message("plain"), "plain");
    }
}
