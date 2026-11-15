//! Self-updating from the GitHub releases, through Velopack.
//!
//! On startup the app checks the latest release. If it's newer, the update downloads in the background and is applied
//! the next time the app starts, or right away with "Restart now". Copies that weren't installed by Velopack (e.g.
//! `cargo tauri dev`) skip all of this.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateManager};

pub const REPO_URL: &str = "https://github.com/SpeakingInBits/GameArtGenerator";
pub const EVENT: &str = "update-status";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum UpdateStatus {
    /// Not installed by Velopack, so it can't update itself.
    NotInstalled,
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
    },
    /// Downloaded; applied on restart.
    Ready {
        version: String,
    },
    Failed {
        message: String,
    },
}

pub struct UpdateState(pub Mutex<UpdateStatus>);

fn manager() -> Result<UpdateManager, velopack::Error> {
    UpdateManager::new(GithubSource::new(REPO_URL, None, false), None, None)
}

fn set(app: &AppHandle, status: UpdateStatus) {
    *app.state::<UpdateState>().0.lock().unwrap() = status.clone();
    let _ = app.emit(EVENT, status);
}

pub fn initial_status() -> UpdateStatus {
    match manager() {
        Ok(_) => UpdateStatus::Idle,
        Err(_) => UpdateStatus::NotInstalled,
    }
}

/// Checks for an update and downloads it, in the background. Progress is sent as `update-status` events.
pub fn check(app: AppHandle) {
    {
        let current = app.state::<UpdateState>().0.lock().unwrap().clone();
        if matches!(
            current,
            UpdateStatus::NotInstalled | UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        ) {
            return;
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(um) = manager() else {
            set(&app, UpdateStatus::NotInstalled);
            return;
        };
        if let Some(pending) = um.get_update_pending_restart() {
            set(
                &app,
                UpdateStatus::Ready {
                    version: pending.Version,
                },
            );
            return;
        }
        set(&app, UpdateStatus::Checking);
        match um.check_for_updates() {
            Ok(UpdateCheck::UpdateAvailable(info)) => {
                let version = info.TargetFullRelease.Version.clone();
                set(
                    &app,
                    UpdateStatus::Downloading {
                        version: version.clone(),
                    },
                );
                match um.download_updates(&info, None) {
                    Ok(()) => set(&app, UpdateStatus::Ready { version }),
                    Err(e) => set(
                        &app,
                        UpdateStatus::Failed {
                            message: format!("Couldn't download version {version}: {e}"),
                        },
                    ),
                }
            }
            Ok(_) => set(&app, UpdateStatus::UpToDate),
            Err(e) => set(
                &app,
                UpdateStatus::Failed {
                    message: format!("Couldn't check for updates: {e}"),
                },
            ),
        }
    });
}

/// Applies the downloaded update and restarts the app. Doesn't return when it succeeds.
pub fn restart_to_update() -> Result<(), String> {
    let um = manager().map_err(|e| e.to_string())?;
    let pending = um
        .get_update_pending_restart()
        .ok_or("No update has been downloaded.")?;
    um.apply_updates_and_restart(pending)
        .map_err(|e| format!("Couldn't apply the update: {e}"))
}

pub fn current_version() -> String {
    manager()
        .map(|um| um.get_current_version_as_string())
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}
