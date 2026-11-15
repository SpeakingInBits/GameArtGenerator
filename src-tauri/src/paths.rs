//! Where the app keeps its data.
//!
//! * Installed with Setup.exe: `%LocalAppData%\SpeakingInBits\GameArtGenerator\`. Velopack installs the app itself
//!   in `%LocalAppData%\GameArtGenerator\` and deletes that whole folder on uninstall, so data lives beside it,
//!   not inside it, and survives both updates and uninstalls.
//! * Portable (Portable.zip): a `data\` folder next to `Update.exe` in the extracted folder. Updates only replace
//!   `current\`, so the data stays, and the whole folder can be moved or copied as one.
//! * `GAME_ART_GENERATOR_DATA_DIR`, when set, overrides both (for development and testing).

use std::path::{Path, PathBuf};

use serde::Serialize;
use velopack::locator::{LocationContext, auto_locate_app_manifest};

pub const DATA_DIR_ENV: &str = "GAME_ART_GENERATOR_DATA_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallMode {
    /// Installed with Setup.exe.
    Installed,
    /// Running from the extracted Portable.zip.
    Portable,
    /// Not packaged by Velopack (e.g. `cargo tauri dev`), or data folder overridden.
    Development,
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub mode: InstallMode,
}

impl AppPaths {
    pub fn resolve() -> Result<Self, String> {
        let located = auto_locate_app_manifest(LocationContext::FromCurrentExe).ok();

        if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|d| !d.is_empty()) {
            return Ok(Self {
                data_dir: PathBuf::from(dir),
                mode: InstallMode::Development,
            });
        }

        if let Some(locator) = &located
            && locator.get_is_portable()
        {
            return Ok(Self {
                data_dir: locator.get_root_dir().join("data"),
                mode: InstallMode::Portable,
            });
        }

        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("The LOCALAPPDATA environment variable isn't set.")?;
        let mode = if located.is_some() {
            InstallMode::Installed
        } else {
            InstallMode::Development
        };
        Ok(Self {
            data_dir: local.join("SpeakingInBits").join("GameArtGenerator"),
            mode,
        })
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("history.db")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn default_images_dir(&self) -> PathBuf {
        self.data_dir.join("images")
    }

    pub fn pasted_dir(&self) -> PathBuf {
        self.data_dir.join("pasted")
    }

    /// Stores paths inside the data folder relative to it, so a portable folder can be moved.
    pub fn to_stored(&self, path: &Path) -> String {
        match path.strip_prefix(&self.data_dir) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }

    pub fn resolve_stored(&self, stored: &str) -> PathBuf {
        let p = PathBuf::from(stored.replace('/', std::path::MAIN_SEPARATOR_STR));
        if p.is_absolute() {
            p
        } else {
            self.data_dir.join(p)
        }
    }
}
