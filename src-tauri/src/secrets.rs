//! The xAI API key. `XAI_API_KEY` takes priority; otherwise the key saved in Windows Credential Manager is used.

use serde::Serialize;

const SERVICE: &str = "SpeakingInBits.GameArtGenerator";
const ACCOUNT: &str = "xai-api-key";
pub const ENV_VAR: &str = "XAI_API_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    Environment,
    CredentialManager,
    None,
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT)
        .map_err(|e| format!("Credential Manager isn't available: {e}"))
}

pub fn api_key() -> (Option<String>, KeySource) {
    if let Ok(key) = std::env::var(ENV_VAR) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return (Some(key), KeySource::Environment);
        }
    }
    match entry().and_then(|e| e.get_password().map_err(|e| e.to_string())) {
        Ok(key) if !key.trim().is_empty() => {
            (Some(key.trim().to_string()), KeySource::CredentialManager)
        }
        _ => (None, KeySource::None),
    }
}

/// Saves the key, or removes the saved one when `key` is empty.
pub fn set_api_key(key: &str) -> Result<(), String> {
    let entry = entry()?;
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
