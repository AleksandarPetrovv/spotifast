//! Runs download jobs on the backend runtime: resolves the songs behind a
//! track, album or playlist, sets the tools up and downloads three songs at
//! a time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Result, anyhow, bail};

use super::download::{self, Covers, Ctx, Song};
use super::{Event, Format, Request, Step};
use crate::api::ApiGateway;
use crate::api::models::Album;
use crate::backend::{ApiRequest, ApiResponse};
use crate::paths::AppDirs;
use crate::player::Engine;

const PARALLEL: usize = 3;

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

pub struct Backend {
    pub api: Arc<ApiGateway>,
    pub engine: Option<Arc<Engine>>,
    pub http: Result<reqwest::Client, String>,
    pub dirs: AppDirs,
    pub emit: Emit,
}

static CANCELS: LazyLock<Mutex<HashMap<u64, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Everything the tweaks keep: tools and local songs.
pub fn root(dirs: &AppDirs) -> PathBuf {
    dirs.state.join("twerkz")
}



pub fn tools_dir(dirs: &AppDirs) -> PathBuf {
    root(dirs).join("tools")
}

pub fn start(request: Request, backend: Backend) {
    match request {
        Request::Cancel { id } => {
            if let Some(flag) = CANCELS.lock().ok().and_then(|held| held.get(&id).cloned()) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        Request::Download {
            id,
            uri,
            name,
            format,
            folder,
        } => {
            let cancel = Arc::new(AtomicBool::new(false));
            if let Ok(mut held) = CANCELS.lock() {
                held.insert(id, cancel.clone());
            }
            tokio::spawn(async move {
                let Some(folder) = folder.await else {
                    (backend.emit)(Event::Dismissed { id });
                    forget(id);
                    return;
                };
                let emit = backend.emit.clone();
                let result = run(id, &uri, &name, format, folder.path().to_path_buf(), cancel, backend).await;
                forget(id);
                if let Err(error) = result {
                    emit(Event::Failed {
                        id,
                        message: format!("{error:#}"),
                    });
                }
            });
        }
        Request::Romanize { uri, lines } => {
            tokio::spawn(async move {
                let result = async {
                    // NetEase's own romaji is written by people; it goes first.
                    if let Some(romaji) = super::lyrics::netease_romaji(&uri, &lines) {
                        return Ok(romaji);
                    }
                    let texts: Vec<String> = lines.into_iter().map(|line| line.text).collect();
                    let script = super::romanize::detect(texts.iter().map(String::as_str))
                        .ok_or_else(|| anyhow!("nothing to romanize"))?;
                    let http = backend.http.clone().map_err(|error| anyhow!(error))?;
                    super::romanize::romanize(&http, script, texts).await
                }
                .await
                .map_err(|error| format!("{error:#}"));
                (backend.emit)(Event::Romanized { uri, result });
            });
        }
    }
}

fn forget(id: u64) {
    if let Ok(mut held) = CANCELS.lock() {
        held.remove(&id);
    }
}

async fn run(
    id: u64,
    uri: &str,
    name: &str,
    format: Format,
    folder: PathBuf,
    cancel: Arc<AtomicBool>,
    backend: Backend,
) -> Result<()> {
    let emit = backend.emit.clone();
    let status = |text: &str| {
        emit(Event::Status {
            id,
            text: text.to_string(),
        })
    };
    let http = backend.http.clone().map_err(|error| anyhow!(error))?;
    status("Getting the songs…");
    let (songs, collection) = resolve(&backend, uri).await?;
    if songs.is_empty() {
        bail!("nothing here can be downloaded");
    }
    let tools = super::tools::ensure(&http, &tools_dir(&backend.dirs), &|text| status(&text)).await?;
    let target = match collection {
        Some(collection) => folder.join(download::sanitize(if collection.is_empty() { name } else { &collection })),
        None => folder,
    };
    let work_dir = backend.dirs.cache.join("twerkz-work").join(id.to_string());
    tokio::fs::create_dir_all(&work_dir).await?;
    let cx = Arc::new(Ctx {
        http,
        tools,
        lyrics_dir: backend.dirs.lyrics_cache_dir(),
        work_dir: work_dir.clone(),
        cancel: cancel.clone(),
    });
    let songs = Arc::new(songs);
    let total = songs.len();
    let next = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let saved = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(Mutex::new(Vec::new()));
    let covers: Covers = Arc::default();
    emit(Event::Progress {
        id,
        done: 0,
        failed: 0,
        total,
        title: String::new(),
        artist: String::new(),
        cover: None,
    });
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..PARALLEL.min(total) {
        let (cx, songs, next, done, saved, failed, covers, target, emit) = (
            cx.clone(),
            songs.clone(),
            next.clone(),
            done.clone(),
            saved.clone(),
            failed.clone(),
            covers.clone(),
            target.clone(),
            emit.clone(),
        );
        workers.spawn(async move {
            loop {
                if cx.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(song) = songs.get(index) else {
                    return;
                };
                emit(Event::Progress {
                    id,
                    done: done.load(Ordering::Relaxed),
                    failed: failed.lock().map(|held| held.len()).unwrap_or(0),
                    total,
                    title: song.title.clone(),
                    artist: song.artists.join(", "),
                    cover: song.cover.clone(),
                });
                match download::song(&cx, song, format, &target, &covers).await {
                    Ok(true) => {
                        saved.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        if cx.cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        log::warn!("download of {} failed: {error:#}", song.title);
                        if let Ok(mut held) = failed.lock() {
                            held.push(format!("{} – {}", song.artists.first().cloned().unwrap_or_default(), song.title));
                        }
                    }
                }
                let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                let failures = failed.lock().map(|held| held.len()).unwrap_or(0);
                emit(Event::Progress {
                    id,
                    done: finished,
                    failed: failures,
                    total,
                    title: String::new(),
                    artist: String::new(),
                    cover: None,
                });
            }
        });
    }
    while workers.join_next().await.is_some() {}
    let _ = tokio::fs::remove_dir_all(&work_dir).await;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    let failed = failed.lock().map(|held| held.clone()).unwrap_or_default();
    let saved = saved.load(Ordering::Relaxed);
    emit(Event::Finished {
        id,
        saved,
        skipped: total - saved - failed.len(),
        failed,
        folder: target,
    });
    Ok(())
}

async fn ask(backend: &Backend, request: ApiRequest) -> ApiResponse {
    crate::backend::handle(&backend.api, backend.engine.as_deref(), request)
        .await
        .0
}

/// The songs behind `uri`, and the album or playlist name when it is one.
async fn resolve(backend: &Backend, uri: &str) -> Result<(Vec<Song>, Option<String>)> {
    let kind = crate::util::uri_kind(uri).unwrap_or("");
    let id = uri.rsplit(':').next().unwrap_or("").to_string();
    match kind {
        "track" => match ask(backend, ApiRequest::Track { id }).await {
            ApiResponse::Track { result, .. } => {
                let track = result.map_err(|error| anyhow!("{error}"))?;
                Ok((Song::from_track(&track, None).into_iter().collect(), None))
            }
            _ => bail!("unexpected answer"),
        },
        _ => bail!("only songs, albums and playlists can be downloaded"),
    }
}
