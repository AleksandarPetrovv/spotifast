//! Spotify songs, albums and playlists as audio files. Each song is matched
//! on YouTube Music (then YouTube) by name and length, downloaded and
//! converted by yt-dlp and ffmpeg, then tagged here with Spotify's details,
//! cover and lyrics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey, TagExt};
use lofty::tag::Tag;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::tools::{Tools, command};
use crate::api::models::Track;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Mp3,
    Wav,
    Ogg,
    Flac,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Mp3, Format::Wav, Format::Ogg, Format::Flac];

    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::Wav => "wav",
            Format::Ogg => "ogg",
            Format::Flac => "flac",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Mp3 => "MP3",
            Format::Wav => "WAV",
            Format::Ogg => "OGG",
            Format::Flac => "FLAC",
        }
    }

    fn ytdlp(self) -> &'static str {
        match self {
            Format::Ogg => "vorbis",
            other => other.ext(),
        }
    }
}

/// What a file is tagged with.
#[derive(Clone, Debug, Default)]
pub struct Song {
    pub uri: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub album_artist: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub date: Option<String>,
    pub cover: Option<String>,
    pub duration_ms: u32,
}

impl Song {
    pub fn from_track(track: &Track, album: Option<&crate::api::models::Album>) -> Option<Song> {
        if track.is_local || track.name.is_empty() {
            return None;
        }
        let album = track.album.as_ref().or(album);
        Some(Song {
            uri: track.uri.clone(),
            title: track.name.clone(),
            artists: track.artists.iter().map(|artist| artist.name.clone()).collect(),
            album: album.map(|album| album.name.clone()).unwrap_or_default(),
            album_artist: album
                .and_then(|album| album.artists.first())
                .map(|artist| artist.name.clone())
                .unwrap_or_default(),
            track: track.track_number,
            disc: track.disc_number,
            date: album.and_then(|album| album.release_date.clone()),
            cover: album.and_then(|album| {
                album
                    .images
                    .iter()
                    .max_by_key(|image| image.width.unwrap_or(0))
                    .map(|image| image.url.clone())
            }),
            duration_ms: track.duration_ms,
        })
    }

    fn artist(&self) -> String {
        self.artists.first().cloned().unwrap_or_default()
    }

    pub fn file_name(&self, format: Format) -> String {
        sanitize(&format!("{} - {}.{}", self.artist(), self.title, format.ext()))
    }
}

pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    let mut out: String = trimmed.chars().take(180).collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

pub struct Ctx {
    pub http: reqwest::Client,
    pub tools: Tools,
    pub lyrics_dir: PathBuf,
    pub work_dir: PathBuf,
    pub cancel: Arc<AtomicBool>,
}

/// Downloads one song into `folder`. `Ok(false)` when it was already there.
pub async fn song(
    cx: &Ctx,
    song: &Song,
    format: Format,
    folder: &Path,
    covers: &tokio::sync::Mutex<HashMap<String, Option<Vec<u8>>>>,
) -> Result<bool> {
    let target = folder.join(song.file_name(format));
    if target.is_file() {
        return Ok(false);
    }
    let file = fetch_audio(cx, song, format).await?;
    let cover = match &song.cover {
        Some(url) => {
            let mut held = covers.lock().await;
            if !held.contains_key(url) {
                let bytes = fetch_bytes(&cx.http, url).await;
                held.insert(url.clone(), bytes);
            }
            held.get(url).cloned().flatten()
        }
        None => None,
    };
    let lyrics = lyrics_text(cx, song).await;
    let tag_file = file.clone();
    let tag_song = song.clone();
    tokio::task::spawn_blocking(move || tag(&tag_file, &tag_song, cover, lyrics))
        .await
        .map_err(|error| anyhow!("{error}"))?
        .unwrap_or_else(|error| log::warn!("could not tag {}: {error:#}", song.title));
    tokio::fs::create_dir_all(folder).await?;
    move_file(&file, &target).await?;
    Ok(true)
}

async fn move_file(from: &Path, to: &Path) -> Result<()> {
    if tokio::fs::rename(from, to).await.is_err() {
        tokio::fs::copy(from, to).await.context("cannot save the song")?;
        let _ = tokio::fs::remove_file(from).await;
    }
    Ok(())
}

async fn fetch_bytes(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let response = http.get(url).send().await.ok()?.error_for_status().ok()?;
    response.bytes().await.ok().map(|bytes| bytes.to_vec())
}

async fn lyrics_text(cx: &Ctx, song: &Song) -> Option<String> {
    let query = crate::lyrics::Query {
        artist: song.artist(),
        title: song.title.clone(),
        album: song.album.clone(),
        duration_ms: song.duration_ms,
    };
    let found = super::lyrics::fetch(&cx.http, &cx.lyrics_dir, &song.uri, &query)
        .await
        .ok()
        .flatten()?;
    if found.instrumental || found.lines.is_empty() {
        return None;
    }
    let text = found
        .lines
        .iter()
        .map(|line| match line.at_ms.filter(|_| found.synced) {
            Some(at) => format!(
                "[{:02}:{:02}.{:02}]{}",
                at / 60_000,
                at / 1000 % 60,
                at % 1000 / 10,
                line.text
            ),
            None => line.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(text)
}

// ---- matching ----------------------------------------------------------------

struct Candidate {
    url: String,
    title: String,
    channel: String,
    duration: Option<f64>,
}

const UNWANTED: &[&str] = &[
    "karaoke", "instrumental", "cover", "remix", "live", "sped up", "slowed", "reverb", "8d",
    "nightcore", "piano", "acoustic", "tutorial", "reaction", "lyrics video", "1 hour",
];

fn unwanted(candidate: &str, wanted: &str) -> bool {
    let candidate = candidate.to_lowercase();
    let wanted = wanted.to_lowercase();
    UNWANTED
        .iter()
        .any(|word| candidate.contains(word) && !wanted.contains(word))
}

async fn search(cx: &Ctx, url: &str) -> Result<Vec<Candidate>> {
    let mut command = command(&cx.tools.ytdlp);
    command
        .args(cx.tools.ytdlp_base())
        .args(["--flat-playlist", "-J", "--playlist-end", "8", url]);
    let output = run(cx, command).await?;
    let json: Value = serde_json::from_slice(&output).context("unexpected search answer")?;
    Ok(json
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(Candidate {
                        url: entry.get("url")?.as_str()?.to_string(),
                        title: entry.get("title")?.as_str()?.to_string(),
                        channel: entry
                            .get("channel")
                            .or_else(|| entry.get("uploader"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        duration: entry.get("duration").and_then(Value::as_f64),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The likeliest uploads, best first.
async fn candidates(cx: &Ctx, song: &Song) -> Vec<String> {
    let title = crate::lyrics::clean_title(&song.title);
    let artist = song.artist();
    let seconds = f64::from(song.duration_ms) / 1000.0;
    let mut picked = Vec::new();
    let music = format!(
        "https://music.youtube.com/search?q={}#songs",
        urlencoding::encode(&format!("{artist} {}", song.title))
    );
    match search(cx, &music).await {
        Ok(found) => {
            for candidate in found {
                if super::similarity(&crate::lyrics::clean_title(&candidate.title), &title) >= 0.75
                    && !unwanted(&candidate.title, &song.title)
                {
                    picked.push(candidate.url);
                }
            }
        }
        Err(error) => log::debug!("youtube music search: {error:#}"),
    }
    let video = format!("ytsearch8:{artist} - {} audio", song.title);
    match search(cx, &video).await {
        Ok(found) => {
            let mut scored: Vec<(f64, String)> = found
                .into_iter()
                .filter(|candidate| !unwanted(&candidate.title, &song.title))
                .filter_map(|candidate| {
                    let lower = candidate.title.to_lowercase();
                    let named = lower.contains(&title.to_lowercase()) || super::similarity(&candidate.title, &title) > 0.6;
                    if !named {
                        return None;
                    }
                    let drift = candidate.duration.map(|d| (d - seconds).abs()).unwrap_or(5.0);
                    if seconds > 0.0 && drift > 15.0 {
                        return None;
                    }
                    let by_artist = super::similarity(&candidate.channel, &artist) > 0.6
                        || candidate.channel.to_lowercase().contains(&artist.to_lowercase())
                        || lower.contains(&artist.to_lowercase());
                    let official = lower.contains("official audio") || candidate.channel.ends_with(" - Topic");
                    let score = (15.0 - drift.min(15.0)) / 15.0
                        + if by_artist { 1.0 } else { 0.0 }
                        + if official { 0.5 } else { 0.0 };
                    Some((score, candidate.url))
                })
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            picked.extend(scored.into_iter().map(|(_, url)| url));
        }
        Err(error) => log::debug!("youtube search: {error:#}"),
    }
    let mut seen = std::collections::HashSet::new();
    picked.retain(|url| seen.insert(url.clone()));
    picked.truncate(5);
    picked
}

/// Downloads the first candidate whose length fits, into the work folder.
async fn fetch_audio(cx: &Ctx, song: &Song, format: Format) -> Result<PathBuf> {
    let found = candidates(cx, song).await;
    if found.is_empty() {
        bail!("no match on YouTube");
    }
    let seconds = song.duration_ms / 1000;
    let mut last_error = None;
    for url in found {
        if cx.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let stem = format!("{:016x}", rand::random::<u64>());
        let mut command = command(&cx.tools.ytdlp);
        command.args(cx.tools.ytdlp_base()).args([
            "--no-playlist",
            "-f",
            "bestaudio/best",
            "-x",
            "--audio-format",
            format.ytdlp(),
            "--postprocessor-args",
            "ExtractAudio:-ar 44100 -ac 2",
            "--audio-quality",
            "0",
            "--retries",
            "2",
        ]);
        if seconds > 0 {
            command.args([
                "--match-filter",
                &format!(
                    "duration >= {} & duration <= {}",
                    seconds.saturating_sub(10),
                    seconds + 10
                ),
            ]);
        }
        command
            .arg("-o")
            .arg(cx.work_dir.join(format!("{stem}.%(ext)s")))
            .arg(&url);
        match run(cx, command).await {
            Ok(_) => {
                let wanted = cx.work_dir.join(format!("{stem}.{}", format.ext()));
                if wanted.is_file() {
                    return Ok(wanted);
                }
                last_error = Some(anyhow!("this upload did not fit"));
            }
            Err(error) => {
                if cx.cancel.load(Ordering::Relaxed) {
                    bail!("cancelled");
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("no downloadable match")))
}

/// Runs a command to completion, or kills it when the job is cancelled.
pub async fn run(cx: &Ctx, mut command: tokio::process::Command) -> Result<Vec<u8>> {
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().context("cannot start yt-dlp")?;
    let cancel = &cx.cancel;
    let watch = async {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    };
    tokio::select! {
        output = child.wait_with_output() => {
            let output = output?;
            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                let line = error.lines().rev().find(|line| line.contains("ERROR")).unwrap_or("download failed");
                bail!("{}", line.trim());
            }
            Ok(output.stdout)
        }
        () = watch => bail!("cancelled"),
    }
}

// ---- tags -----------------------------------------------------------------------

fn tag(path: &Path, song: &Song, cover: Option<Vec<u8>>, lyrics: Option<String>) -> Result<()> {
    let mut tagged = lofty::read_from_path(path)?;
    let kind = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(kind));
    }
    let tag = tagged.primary_tag_mut().context("no tag")?;
    tag.set_title(song.title.clone());
    tag.set_artist(song.artists.join(", "));
    if !song.album.is_empty() {
        tag.set_album(song.album.clone());
    }
    if !song.album_artist.is_empty() {
        tag.insert_text(ItemKey::AlbumArtist, song.album_artist.clone());
    }
    if let Some(track) = song.track {
        tag.set_track(track);
    }
    if let Some(disc) = song.disc {
        tag.set_disk(disc);
    }
    if let Some(date) = &song.date {
        tag.insert_text(ItemKey::RecordingDate, date.clone());
    }
    if let Some(lyrics) = lyrics {
        tag.insert_text(ItemKey::Lyrics, lyrics);
    }
    if let Some(bytes) = cover {
        let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(mime), None, bytes));
    }
    // Spotify only shows covers from ID3v2.3.
    tag.save_to_path(path, WriteOptions::default().use_id3v23(true))?;
    Ok(())
}

/// Shared by every job: songs covers, keyed by URL.
pub type Covers = Arc<tokio::sync::Mutex<HashMap<String, Option<Vec<u8>>>>>;

#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore = "network, downloads tools"]
    async fn downloads_and_tags_one_song() {
        let dirs = crate::paths::AppDirs::discover();
        let http = reqwest::Client::new();
        let tools = super::super::tools::ensure(&http, &super::super::jobs::tools_dir(&dirs), &|text| eprintln!("{text}"))
            .await
            .unwrap();
        eprintln!("{tools:?}");
        let out = std::env::temp_dir().join("twerkz-live");
        let cx = Ctx {
            http,
            tools,
            lyrics_dir: dirs.lyrics_cache_dir(),
            work_dir: out.join("work"),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        std::fs::create_dir_all(&cx.work_dir).unwrap();
        let song = Song {
            uri: "spotify:track:0VjIjW4GlUZAMYd2vXMi3b".into(),
            title: "Blinding Lights".into(),
            artists: vec!["The Weeknd".into()],
            album: "After Hours".into(),
            album_artist: "The Weeknd".into(),
            track: Some(9),
            disc: Some(1),
            date: Some("2020-03-20".into()),
            cover: Some("https://i.scdn.co/image/ab67616d0000b2738863bc11d2aa12b54f5aeb36".into()),
            duration_ms: 200_040,
        };
        let started = std::time::Instant::now();
        let covers = tokio::sync::Mutex::new(HashMap::new());
        let saved = song_download(&cx, &song, &out, &covers).await;
        eprintln!("result {saved:?} in {:?}", started.elapsed());
        let path = out.join(song.file_name(Format::Mp3));
        let tagged = lofty::read_from_path(&path).unwrap();
        let tag = tagged.primary_tag().unwrap();
        eprintln!(
            "title={:?} artist={:?} album={:?} pictures={} lyrics={} bytes={}",
            tag.title(),
            tag.artist(),
            tag.album(),
            tag.pictures().len(),
            tag.get_string(&ItemKey::Lyrics).map(|l| l.len()).unwrap_or(0),
            std::fs::metadata(&path).unwrap().len()
        );
    }

    async fn song_download(
        cx: &Ctx,
        s: &Song,
        out: &Path,
        covers: &tokio::sync::Mutex<HashMap<String, Option<Vec<u8>>>>,
    ) -> Result<bool> {
        song(cx, s, Format::Mp3, out, covers).await
    }
}
