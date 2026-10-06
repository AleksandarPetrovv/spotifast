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

/// Imported songs, played by spotifast and matched by Spotify.
pub fn local_songs_dir(dirs: &AppDirs) -> PathBuf {
    root(dirs).join("local songs")
}

/// The local songs folder exists and is one of the scanned local folders, so
/// imports index and play like any other local file. The folders Spotify's
/// desktop app plays local files from are offered once too, so songs it
/// added to playlists play here as well.
pub fn ensure_local_songs_folder(dirs: &AppDirs, settings: &mut crate::settings::Settings) {
    let folder = local_songs_dir(dirs);
    if std::fs::create_dir_all(&folder).is_err() {
        return;
    }
    let offered_file = root(dirs).join("offered-folders.txt");
    let offered = std::fs::read_to_string(&offered_file).unwrap_or_default();
    let mut offered: Vec<String> = offered.lines().map(str::to_string).collect();
    let mut changed = false;
    let mut wanted = vec![folder.to_string_lossy().into_owned()];
    wanted.extend(spotify_folders());
    for path in wanted {
        if offered.contains(&path) {
            continue;
        }
        offered.push(path.clone());
        if !settings.local_folders.contains(&path) {
            settings.local_folders.push(path);
            changed = true;
        }
    }
    let _ = std::fs::write(&offered_file, offered.join("\n"));
    // A legacy proxy password is migrated before settings may be rewritten.
    if changed && !settings.proxy_password_legacy {
        settings.save(&dirs.settings_file());
    }
}

/// The folders holding the local files Spotify's desktop app knows of.
fn spotify_folders() -> Vec<String> {
    let Some(base) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let users = if cfg!(windows) {
        base.config_dir().join("Spotify").join("Users")
    } else if cfg!(target_os = "macos") {
        base.data_dir().join("Spotify").join("Users")
    } else {
        base.config_dir().join("spotify").join("Users")
    };
    let mut folders: Vec<String> = Vec::new();
    for user in std::fs::read_dir(users).into_iter().flatten().flatten() {
        let Ok(bank) = std::fs::read(user.path().join("local-files.bnk")) else {
            continue;
        };
        for path in bank_paths(&bank) {
            if let Some(parent) = std::path::Path::new(&path).parent()
                && parent.is_dir()
            {
                let parent = parent.to_string_lossy().into_owned();
                if !folders.contains(&parent) {
                    folders.push(parent);
                }
            }
        }
    }
    folders
}

/// The file paths written in a Spotify local files bank.
fn bank_paths(bank: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut at = 0;
    while at + 3 < bank.len() {
        let windows = bank[at].is_ascii_alphabetic() && bank[at + 1] == b':' && bank[at + 2] == b'\\';
        let unix = bank[at] == b'/' && at > 0 && bank[at - 1] < 0x20 && bank[at + 1].is_ascii_alphanumeric();
        if !windows && !unix {
            at += 1;
            continue;
        }
        let end = bank[at..]
            .iter()
            .position(|byte| *byte < 0x20)
            .map_or(bank.len(), |offset| at + offset);
        let path = String::from_utf8_lossy(&bank[at..end]).into_owned();
        if std::path::Path::new(&path).extension().is_some() {
            paths.push(path);
        }
        at = end;
    }
    paths
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
        Request::AddLocal {
            id,
            playlist_id,
            uris,
        } => {
            tokio::spawn(async move {
                let count = uris.len();
                let result = async {
                    let session = backend
                        .engine
                        .as_ref()
                        .map(|engine| engine.session().clone())
                        .ok_or_else(|| anyhow!("connect to Spotify first"))?;
                    super::playlist::append(&session, &playlist_id, &uris).await?;
                    Ok::<_, anyhow::Error>(if count == 1 {
                        "Added 1 song".to_string()
                    } else {
                        format!("Added {count} songs")
                    })
                }
                .await
                .map_err(|error| format!("{error:#}"));
                (backend.emit)(Event::AddedLocal {
                    id,
                    playlist_id,
                    result,
                });
            });
        }
        Request::ChooseEmojiFont { file } => {
            tokio::spawn(async move {
                let result = match file.await {
                    Some(file) => {
                        let path = file.path().to_path_buf();
                        tokio::task::spawn_blocking(move || super::emoji::choose(&path))
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|chosen| chosen.map(Some).map_err(|error| error.to_string()))
                    }
                    None => Ok(None),
                };
                (backend.emit)(Event::EmojiFont { result });
            });
        }
        Request::ImportPreview { id, url } => {
            tokio::spawn(async move {
                let result = async {
                    let cx = context(&backend, id, Arc::default()).await?;
                    let found = super::import::find(&cx, &url).await;
                    let _ = tokio::fs::remove_dir_all(&cx.work_dir).await;
                    found
                }
                .await
                .map_err(|error| format!("{error:#}"));
                (backend.emit)(Event::ImportPreview { id, result });
            });
        }
        Request::Import {
            id,
            playlist_id,
            songs,
        } => {
            let cancel = Arc::new(AtomicBool::new(false));
            if let Ok(mut held) = CANCELS.lock() {
                held.insert(id, cancel.clone());
            }
            tokio::spawn(async move {
                let emit = backend.emit.clone();
                let total = songs.len();
                let say = |index: usize, step: Step| {
                    emit(Event::Imported {
                        id,
                        playlist_id: playlist_id.clone(),
                        index,
                        total,
                        step,
                    })
                };
                let result = import(id, &playlist_id, songs, cancel.clone(), &backend, &say).await;
                forget(id);
                say(
                    total,
                    Step::Finished(
                        result
                            .map(|()| cancel.load(Ordering::Relaxed))
                            .map_err(|error| format!("{error:#}")),
                    ),
                );
            });
        }
    }
}

/// Downloads the songs a few at a time and adds each to the playlist as
/// soon as the ones before it are in, so the playlist keeps their order.
async fn import(
    id: u64,
    playlist_id: &str,
    songs: Vec<super::import::Preview>,
    cancel: Arc<AtomicBool>,
    backend: &Backend,
    say: &(dyn Fn(usize, Step) + Sync),
) -> Result<()> {
    let session = backend
        .engine
        .as_ref()
        .map(|engine| engine.session().clone())
        .ok_or_else(|| anyhow!("connect to Spotify first"))?;
    let cx = Arc::new(context(backend, id, cancel.clone()).await?);
    let folder = local_songs_dir(&backend.dirs);
    let songs = Arc::new(songs);
    let mut pending = std::collections::VecDeque::new();
    let mut next = 0;
    loop {
        while pending.len() < PARALLEL && next < songs.len() && !cancel.load(Ordering::Relaxed) {
            let (cx, songs, folder, index) = (cx.clone(), songs.clone(), folder.clone(), next);
            pending.push_back((
                index,
                tokio::spawn(async move {
                    let song = &songs[index];
                    super::import::download(&cx, song, &song.title, &song.artist, &folder).await
                }),
            ));
            next += 1;
        }
        let Some((index, handle)) = pending.pop_front() else {
            break;
        };
        let song = &songs[index];
        say(
            index,
            Step::Working {
                title: song.title.clone(),
                artist: song.artist.clone(),
                cover: song.thumbnail.clone(),
            },
        );
        let downloaded = handle.await.map_err(|error| anyhow!("{error}")).and_then(|file| file);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let added = async {
            let file = downloaded?;
            let uri = crate::localfiles::uri_of(&file).map_err(|error| anyhow!(error))?;
            super::playlist::append(&session, playlist_id, &[uri]).await
        }
        .await;
        match added {
            Ok(()) => say(index, Step::Added),
            Err(error) => {
                log::warn!("import of {} failed: {error:#}", song.url);
                say(index, Step::Failed(format!("{error:#}")));
            }
        }
    }
    for (_, handle) in pending {
        handle.abort();
    }
    let _ = tokio::fs::remove_dir_all(&cx.work_dir).await;
    Ok(())
}

/// Tools set up and a fresh work folder for job `id`.
async fn context(backend: &Backend, id: u64, cancel: Arc<AtomicBool>) -> Result<Ctx> {
    let http = backend.http.clone().map_err(|error| anyhow!(error))?;
    let emit = backend.emit.clone();
    let tools = super::tools::ensure(&http, &tools_dir(&backend.dirs), &move |text| {
        emit(Event::ImportStatus { id, text })
    })
    .await?;
    let work_dir = backend.dirs.cache.join("twerkz-work").join(id.to_string());
    tokio::fs::create_dir_all(&work_dir).await?;
    Ok(Ctx {
        http,
        tools,
        lyrics_dir: backend.dirs.lyrics_cache_dir(),
        work_dir,
        cancel,
    })
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
        "album" => {
            let album: Album = match ask(backend, ApiRequest::Album { id: id.clone() }).await {
                ApiResponse::Album { result, .. } => result.map_err(|error| anyhow!("{error}"))?,
                _ => bail!("unexpected answer"),
            };
            let mut songs = Vec::new();
            let mut offset = 0;
            loop {
                let page = match ask(
                    backend,
                    ApiRequest::AlbumTracks {
                        id: id.clone(),
                        offset,
                        generation: 0,
                    },
                )
                .await
                {
                    ApiResponse::AlbumTracks { result, .. } => result.map_err(|error| anyhow!("{error}"))?,
                    _ => bail!("unexpected answer"),
                };
                let count = page.items.len() as u32;
                songs.extend(page.items.iter().filter_map(|track| Song::from_track(track, Some(&album))));
                offset += count;
                if count == 0 || offset >= page.total {
                    break;
                }
            }
            Ok((songs, Some(album.name)))
        }
        "playlist" => {
            let name = match ask(
                backend,
                ApiRequest::Playlist {
                    id: id.clone(),
                    generation: 0,
                },
            )
            .await
            {
                ApiResponse::Playlist { result, .. } => result.map(|playlist| playlist.name).unwrap_or_default(),
                _ => String::new(),
            };
            let mut songs = Vec::new();
            let mut offset = 0;
            loop {
                let page = match ask(
                    backend,
                    ApiRequest::PlaylistItems {
                        id: id.clone(),
                        offset,
                        generation: 0,
                    },
                )
                .await
                {
                    ApiResponse::PlaylistItems { result, .. } => result.map_err(|error| anyhow!("{error}"))?,
                    _ => bail!("unexpected answer"),
                };
                let count = page.items.len() as u32;
                for row in &page.items {
                    if let Some(crate::api::models::PlayableItem::Track(track)) = row.playable()
                        && let Some(song) = Song::from_track(track, None)
                    {
                        songs.push(song);
                    }
                }
                offset += count;
                if count == 0 || offset >= page.total {
                    break;
                }
            }
            Ok((songs, Some(name)))
        }
        _ => bail!("only songs, albums and playlists can be downloaded"),
    }
}
