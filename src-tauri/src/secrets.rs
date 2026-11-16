//! API keys. For each service the environment variable takes priority; otherwise the key saved in Windows Credential
//! Manager is used.

use serde::{Deserialize, Serialize};

const SERVICE: &str = "SpeakingInBits.GameArtGenerator";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    Environment,
    CredentialManager,
    None,
}

/// Whose key: xAI (images), Tripo (3D models) or ElevenLabs (audio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Service {
    #[default]
    Xai,
    Tripo,
    #[serde(rename = "elevenlabs")]
    ElevenLabs,
}

impl Service {
    fn account(self) -> &'static str {
        match self {
            Service::Xai => "xai-api-key",
            Service::Tripo => "tripo-api-key",
            Service::ElevenLabs => "elevenlabs-api-key",
        }
    }

    pub fn env_var(self) -> &'static str {
        match self {
            Service::Xai => "XAI_API_KEY",
            Service::Tripo => "TRIPO_API_KEY",
            Service::ElevenLabs => "ELEVENLABS_API_KEY",
        }
    }
}

fn entry(service: Service) -> Result<keyring::Entry, String> {
    entry_for(service.account())
}

fn entry_for(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account)
        .map_err(|e| format!("Credential Manager isn't available: {e}"))
}

/// The access token for a self-hosted audio server, if one is saved (servers reached through an SSH tunnel usually
/// don't need one).
pub fn server_token(server_id: &str) -> Option<String> {
    entry_for(&format!("audio-server-{server_id}"))
        .ok()?
        .get_password()
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Saves a server's token, or removes it when `token` is empty.
pub fn set_server_token(server_id: &str, token: &str) -> Result<(), String> {
    let entry = entry_for(&format!("audio-server-{server_id}"))?;
    if token.trim().is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Couldn't remove the server token: {e}")),
        }
    } else {
        entry
            .set_password(token.trim())
            .map_err(|e| format!("Couldn't save the server token: {e}"))
    }
}

pub fn api_key(service: Service) -> (Option<String>, KeySource) {
    if let Ok(key) = std::env::var(service.env_var()) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return (Some(key), KeySource::Environment);
        }
    }
    match entry(service).and_then(|e| e.get_password().map_err(|e| e.to_string())) {
        Ok(key) if !key.trim().is_empty() => {
            (Some(key.trim().to_string()), KeySource::CredentialManager)
        }
        _ => (None, KeySource::None),
    }
}

/// Saves the key, or removes the saved one when `key` is empty.
pub fn set_api_key(service: Service, key: &str) -> Result<(), String> {
    let entry = entry(service)?;
    let key = key.trim();
    if key.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Couldn't remove the saved key: {e}")),
        }
    } else {
        entry
            .set_password(key)
            .map_err(|e| format!("Couldn't save the key: {e}"))
    }
}
