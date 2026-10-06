//! The playlist list from the last run, shown at once while Spotify's
//! shared app, which serves the full list, answers. The fresh list replaces
//! it whole once every page has arrived, so nothing shrinks in between.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::models::Playlist;
use crate::paths::AppDirs;

#[derive(Default)]
pub struct LibraryCache {
    /// True while the cached list is on screen and fresh pages are being
    /// gathered behind it.
    pub showing_cached: bool,
    pub pending: Vec<Playlist>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    account: Option<String>,
    playlists: Vec<Playlist>,
}

fn file(dirs: &AppDirs) -> PathBuf {
    dirs.cache.join("twerkz-library.json")
}

/// The stored list, unless it belongs to another account.
pub fn load(dirs: &AppDirs, account: Option<&str>) -> Option<Vec<Playlist>> {
    let text = std::fs::read_to_string(file(dirs)).ok()?;
    let stored: Stored = serde_json::from_str(&text).ok()?;
    if let (Some(account), Some(owner)) = (account, stored.account.as_deref())
        && account != owner
    {
        return None;
    }
    (!stored.playlists.is_empty()).then_some(stored.playlists)
}

pub fn store(dirs: &AppDirs, account: Option<&str>, playlists: &[Playlist]) {
    let stored = Stored {
        account: account.map(str::to_string),
        playlists: playlists.to_vec(),
    };
    let path = file(dirs);
    std::thread::spawn(move || {
        let Ok(text) = serde_json::to_string(&stored) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let partial = path.with_extension("tmp");
        if std::fs::write(&partial, text).is_ok() {
            let _ = std::fs::rename(&partial, &path);
        }
    });
}
