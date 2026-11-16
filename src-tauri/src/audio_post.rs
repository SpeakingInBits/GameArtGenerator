//! Audio clean-up for game use: decode whatever a provider returned (MP3, FLAC, WAV, Ogg Vorbis), trim silence
//! from the ends, bring the peak to -1 dBFS, and save 16-bit WAV, which every engine imports.

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Below this level (about -50 dBFS) counts as silence when trimming.
const SILENCE: f32 = 0.003;
/// Kept before the first sound and after the last, so attacks and tails aren't clipped.
const PAD_BEFORE_S: f64 = 0.01;
const PAD_AFTER_S: f64 = 0.05;
/// A short fade where a tail was cut, so it doesn't click.
const FADE_S: f64 = 0.005;
/// -1 dBFS.
const TARGET_PEAK: f32 = 0.891;

#[derive(Debug, Clone)]
pub struct Decoded {
    /// Interleaved samples in -1..1.
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u16,
}

impl Decoded {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }

    fn frame_peak(&self, frame: usize) -> f32 {
        let c = self.channels as usize;
        self.samples[frame * c..frame * c + c]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Cuts silence from both ends, keeping a little padding.
    pub fn trim_silence(&mut self) {
        let frames = self.frames();
        let Some(first) = (0..frames).find(|&f| self.frame_peak(f) > SILENCE) else {
            return; // all silence: leave it for the user to judge
        };
        let last = (0..frames)
            .rev()
            .find(|&f| self.frame_peak(f) > SILENCE)
            .unwrap_or(first);
        let rate = self.rate as f64;
        let start = first.saturating_sub((PAD_BEFORE_S * rate) as usize);
        let end = (last + 1 + (PAD_AFTER_S * rate) as usize).min(frames);
        let c = self.channels as usize;
        let cut_tail = end < frames;
        self.samples = self.samples[start * c..end * c].to_vec();
        if cut_tail {
            let fade = ((FADE_S * rate) as usize).min(self.frames());
            let n = self.frames();
            for i in 0..fade {
                let gain = i as f32 / fade as f32;
                for ch in 0..c {
                    self.samples[(n - 1 - i) * c + ch] *= gain;
                }
            }
        }
    }

    /// Scales the clip so its loudest sample is at -1 dBFS.
    pub fn normalize(&mut self) {
        let peak = self.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > 1e-6 {
            let gain = TARGET_PEAK / peak;
            self.samples.iter_mut().for_each(|s| *s *= gain);
        }
    }

    pub fn to_wav(&self) -> Result<Vec<u8>, String> {
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut buf, spec).map_err(|e| e.to_string())?;
            for s in &self.samples {
                w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)
                    .map_err(|e| e.to_string())?;
            }
            w.finalize().map_err(|e| e.to_string())?;
        }
        Ok(buf.into_inner())
    }
}

/// Decodes a whole clip. `ext` is a hint for the container (`mp3`, `flac`, `wav`, `ogg`).
pub fn decode(bytes: &[u8], ext: &str) -> Result<Decoded, String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(ext);
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("Couldn't read the {ext} audio: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("The file has no audio track.")?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Can't decode this audio: {e}"))?;

    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("Couldn't read the audio: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                channels = spec.channels.count() as u16;
                let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                samples.extend_from_slice(buf.samples());
            }
            // A damaged packet: skip it, as players do.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Couldn't decode the audio: {e}")),
        }
    }
    if samples.is_empty() {
        return Err("The audio is empty.".into());
    }
    Ok(Decoded {
        samples,
        rate,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 s of stereo at 1 kHz: silence, a 0.25 s tone at half volume, then silence.
    fn clip() -> Decoded {
        let rate = 1000;
        let mut samples = Vec::new();
        for i in 0..rate {
            let v = if (300..550).contains(&i) {
                if i % 2 == 0 { 0.5 } else { -0.5 }
            } else {
                0.0
            };
            samples.extend([v, v]);
        }
        Decoded {
            samples,
            rate: rate as u32,
            channels: 2,
        }
    }

    #[test]
    fn trims_and_normalizes() {
        let mut d = clip();
        d.trim_silence();
        // 10 ms before + 250 ms of tone + 50 ms after.
        assert_eq!(d.frames(), 10 + 250 + 50);
        d.normalize();
        let peak = d.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - TARGET_PEAK).abs() < 1e-4);
        // The fade ends at silence.
        assert_eq!(*d.samples.last().unwrap(), 0.0);
    }

    #[test]
    fn leaves_all_silence_alone() {
        let mut d = Decoded {
            samples: vec![0.0; 200],
            rate: 100,
            channels: 2,
        };
        d.trim_silence();
        d.normalize();
        assert_eq!(d.frames(), 100);
    }

    #[test]
    fn wav_round_trip() {
        let d = clip();
        let wav = d.to_wav().unwrap();
        let back = decode(&wav, "wav").unwrap();
        assert_eq!((back.rate, back.channels, back.frames()), (1000, 2, 1000));
        assert!((back.duration() - 1.0).abs() < 1e-9);
        assert!((back.samples[600] - 0.5).abs() < 1e-3);
    }
}

/// End to end against a running audio server: `AUDIO_SERVER_URL=http://127.0.0.1:8190 AUDIO_SERVER_TOKEN=…
/// cargo test -- --ignored live_audio_server` (start it with `python audio-server/server.py --models demo`).
#[cfg(test)]
mod live {
    use crate::audio::AudioServerClient;

    #[tokio::test]
    #[ignore]
    async fn live_audio_server() {
        let url = std::env::var("AUDIO_SERVER_URL").expect("set AUDIO_SERVER_URL");
        let client = AudioServerClient::new(&url, std::env::var("AUDIO_SERVER_TOKEN").ok());
        let info = client.info().await.unwrap();
        let model = info
            .models
            .iter()
            .find(|m| m.kind == "sfx")
            .expect("an sfx model");
        let (clips, seed) = client
            .generate(&model.id, "laser zap", None, 2.0, Some(11), 2)
            .await
            .unwrap();
        assert_eq!((clips.len(), seed), (2, Some(11)));
        let mut d = super::decode(&clips[0].bytes, &clips[0].ext).unwrap();
        let before = d.duration();
        assert!((before - 2.0).abs() < 0.05, "{before}");
        d.trim_silence();
        d.normalize();
        assert!(d.duration() < before, "the leading silence was trimmed");
        let peak = d.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - super::TARGET_PEAK).abs() < 1e-3);
        // A wrong token is refused with the server's message.
        let wrong = AudioServerClient::new(&url, Some("wrong".into()))
            .info()
            .await
            .unwrap_err();
        assert!(wrong.contains("Wrong or missing token"), "{wrong}");
    }
}

/// End to end against ComfyUI (or a stand-in): `COMFY_URL=http://127.0.0.1:8188 cargo test -- --ignored live_comfy`.
/// Needs a server whose checkpoints match the workflow, so it's for checking a setup by hand.
#[cfg(test)]
mod live_comfy {
    use std::time::Duration;

    use serde_json::json;

    use crate::audio::{ComfyClient, detect_mapping, fill_workflow};

    #[tokio::test]
    #[ignore]
    async fn live_comfy() {
        let url = std::env::var("COMFY_URL").expect("set COMFY_URL");
        let client = ComfyClient::new(&url, None);
        assert!(!client.describe().await.unwrap().is_empty());
        let w = json!({
            "3": { "class_type": "KSampler", "inputs": { "seed": 1, "positive": ["6", 0], "negative": ["7", 0] } },
            "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "" } },
            "7": { "class_type": "CLIPTextEncode", "inputs": { "text": "" } },
            "11": { "class_type": "EmptyLatentAudio", "inputs": { "seconds": 47.6 } },
            "19": { "class_type": "SaveAudio", "inputs": { "audio": ["12", 0] } }
        });
        let filled = fill_workflow(
            &w,
            &detect_mapping(&w),
            "rain on a tin roof",
            None,
            Some(1.5),
            42,
        );
        let clips = client.run(&filled, Duration::from_secs(60)).await.unwrap();
        assert_eq!(clips.len(), 1);
        let d = super::decode(&clips[0].bytes, &clips[0].ext).unwrap();
        assert!((d.duration() - 1.5).abs() < 0.05, "{}", d.duration());

        let mut broken = w.clone();
        broken["9"] = json!({ "class_type": "Broken", "inputs": {} });
        let err = client
            .run(&broken, Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(err.contains("Value not in list"), "{err}");
    }
}
