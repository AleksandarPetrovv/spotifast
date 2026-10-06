//! YouTube and SoundCloud songs into Spotify playlists: downloaded as MP3
//! into the local songs folder, tagged so Spotify can match the file, then
//! added to the playlist as a local song.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, TagExt};
use lofty::tag::Tag;
use serde_json::Value;

use super::download::{Ctx, run, sanitize};
use super::tools::command;

/// One song behind a link.
#[derive(Clone, Debug, Default)]
pub struct Preview {
    pub url: String,
    pub title: String,
    pub artist: String,
    /// "YouTube" or "SoundCloud", written as the album.
    pub source: String,
    pub seconds: u64,
    pub thumbnail: Option<String>,
}

/// A playlist, album or set: its songs in order.
#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub title: String,
    pub source: String,
    pub cover: Option<String>,
    pub entries: Vec<Preview>,
    /// A YouTube mix, cut to its first 50 songs.
    pub mix: bool,
}

#[derive(Clone, Debug)]
pub enum Found {
    Song(Preview),
    Collection(Collection),
}

const MAX_SONGS: usize = 2000;
const MIX_SONGS: usize = 50;

/// YouTube's endless mixes and radios. A song opened from one is that song.
fn is_mix(list: &str) -> bool {
    list.starts_with("RD") && !list.starts_with("RDCLAK")
}

/// The link made whole: one song, or a whole playlist, album or set.
/// A song played inside a mix is just that song.
pub fn normalize_link(text: &str) -> Option<String> {
    let text = text.trim();
    let raw = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let url = reqwest::Url::parse(&raw).ok()?;
    let host = url.host_str()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let query = |key: &str| {
        url.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
    };
    match host {
        "youtube.com" | "music.youtube.com" | "youtu.be" => {
            let path = url.path();
            let mut video = if host == "youtu.be" {
                path.trim_matches('/').split('/').next().map(str::to_string)
            } else {
                query("v")
            };
            for prefix in ["/shorts/", "/live/", "/embed/"] {
                if let Some(rest) = path.strip_prefix(prefix) {
                    video = rest.split('/').next().map(str::to_string);
                }
            }
            let video = video.filter(|id| id.len() == 11);
            match (video, query("list")) {
                (Some(video), Some(list)) if is_mix(&list) => {
                    Some(format!("https://www.youtube.com/watch?v={video}"))
                }
                (_, Some(list)) => Some(format!("https://www.youtube.com/playlist?list={list}")),
                (Some(video), None) => Some(format!("https://www.youtube.com/watch?v={video}")),
                (None, None) => None,
            }
        }
        "soundcloud.com" => {
            if url.path().trim_matches('/').is_empty() {
                return None;
            }
            // A song shared from a set names the set in its query; the
            // link is still the one song.
            let mut url = url.clone();
            url.set_query(None);
            url.set_fragment(None);
            Some(url.to_string())
        }
        "on.soundcloud.com" | "snd.sc" => Some(url.to_string()),
        _ => None,
    }
}

/// What the link holds: one song, or a list of them.
pub async fn find(cx: &Ctx, url: &str) -> Result<Found> {
    let soundcloud = url.contains("soundcloud");
    let mix = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(name, _)| name == "list")
                .map(|(_, list)| is_mix(&list))
        })
        .unwrap_or(false);
    let limit = if mix { MIX_SONGS } else { MAX_SONGS };
    let mut command = command(&cx.tools.ytdlp);
    command
        .args(cx.tools.ytdlp_base())
        .args(["--yes-playlist", "--skip-download", "--playlist-end"])
        .arg(limit.to_string());
    // SoundCloud's flat lists carry no names, so its sets are read whole.
    if !soundcloud {
        command.arg("--flat-playlist");
    }
    command.args(["-J", url]);
    let output = run(cx, command).await?;
    let json: Value = serde_json::from_slice(&output).context("unexpected answer from yt-dlp")?;
    if !matches!(json.get("_type").and_then(Value::as_str), Some("playlist" | "multi_video")) {
        return song_of(&json, url).map(Found::Song);
    }
    let source = source_of(&json);
    let mut entries = Vec::new();
    collect(&json, source, &mut entries);
    entries.truncate(limit);
    if entries.is_empty() {
        bail!("nothing in this collection can be added");
    }
    let cover = thumbnail_of(&json).or_else(|| entries.first().and_then(|entry| entry.thumbnail.clone()));
    Ok(Found::Collection(Collection {
        title: text(&json, &["title"]).unwrap_or("Collection").to_string(),
        source: source.to_string(),
        cover,
        entries,
        mix,
    }))
}

fn text<'a>(json: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| json.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn source_of(json: &Value) -> &'static str {
    let extractor = text(json, &["extractor_key", "ie_key", "extractor"]).unwrap_or("");
    if extractor.to_lowercase().contains("soundcloud") {
        "SoundCloud"
    } else {
        "YouTube"
    }
}

fn youtube_id(json: &Value) -> Option<&str> {
    text(json, &["id"]).filter(|id| {
        id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// The best JPEG or PNG picture of an upload. YouTube's own thumbnails are
/// asked for by name, so no WebP is picked.
fn thumbnail_of(json: &Value) -> Option<String> {
    if source_of(json) == "YouTube"
        && let Some(id) = youtube_id(json)
    {
        return Some(format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"));
    }
    json.get("thumbnails")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .rev()
                .filter_map(|thumb| thumb.get("url")?.as_str())
                .find(|url| {
                    let path = url.split('?').next().unwrap_or(url).to_lowercase();
                    path.ends_with(".jpg") || path.ends_with(".jpeg") || path.ends_with(".png")
                })
        })
        .or_else(|| json.get("thumbnail").and_then(Value::as_str))
        .map(str::to_string)
}

fn names_of(json: &Value, fallback: &str) -> (String, String) {
    let uploader = text(json, &["artist", "creator", "uploader", "channel"])
        .unwrap_or("")
        .trim_end_matches(" - Topic");
    match text(json, &["track"]) {
        Some(track) if !uploader.is_empty() => (track.to_string(), uploader.to_string()),
        _ => clean_name(text(json, &["title"]).unwrap_or(fallback), uploader),
    }
}

fn song_of(json: &Value, url: &str) -> Result<Preview> {
    let live = matches!(
        json.get("live_status").and_then(Value::as_str),
        Some("is_live" | "is_upcoming")
    ) || json.get("is_live").and_then(Value::as_bool) == Some(true);
    if live {
        bail!("use a finished upload, not a live stream");
    }
    let (title, artist) = names_of(json, "Song");
    Ok(Preview {
        url: text(json, &["webpage_url"]).unwrap_or(url).to_string(),
        title,
        artist,
        source: source_of(json).to_string(),
        seconds: json.get("duration").and_then(Value::as_f64).unwrap_or(0.0) as u64,
        thumbnail: thumbnail_of(json),
    })
}

/// The songs of a list in order, lists inside it skipped.
fn collect(json: &Value, source: &str, out: &mut Vec<Preview>) {
    let Some(entries) = json.get("entries").and_then(Value::as_array) else {
        return;
    };
    for entry in entries {
        if out.len() >= MAX_SONGS {
            return;
        }
        if entry.get("entries").is_some() {
            collect(entry, source, out);
            continue;
        }
        let title = text(entry, &["title"]).unwrap_or("");
        if matches!(title, "[Private video]" | "[Deleted video]") {
            continue;
        }
        if matches!(
            entry.get("live_status").and_then(Value::as_str),
            Some("is_live" | "is_upcoming")
        ) {
            continue;
        }
        let url = match (source, youtube_id(entry)) {
            ("YouTube", Some(id)) => format!("https://www.youtube.com/watch?v={id}"),
            ("YouTube", None) => continue,
            _ => match text(entry, &["webpage_url", "url"]) {
                Some(url) if !url.contains("/sets/") => url.to_string(),
                _ => continue,
            },
        };
        let (title, artist) = names_of(entry, "");
        out.push(Preview {
            url,
            title,
            artist,
            source: source.to_string(),
            seconds: entry.get("duration").and_then(Value::as_f64).unwrap_or(0.0) as u64,
            thumbnail: thumbnail_of(entry),
        });
    }
}

/// "Artist - Title (Official Video)" as a title and an artist.
pub fn clean_name(raw: &str, uploader: &str) -> (String, String) {
    const NOISE: &[&str] = &[
        "official music video", "official video", "official audio", "official visualizer",
        "official visualiser", "official lyric video", "official lyrics video", "lyric video",
        "lyrics video", "music video", "visualizer", "visualiser", "lyrics", "lyric", "audio",
        "video", "hd", "hq", "4k", "1080p", "720p", "official",
    ];
    let mut cleaned = String::new();
    let mut rest = raw;
    while let Some(open) = rest.find(['(', '[']) {
        cleaned.push_str(&rest[..open]);
        let close_char = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(close) = rest[open..].find(close_char) else {
            cleaned.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let inside = rest[open + 1..open + close].to_lowercase();
        let mut stripped = inside.clone();
        for word in NOISE {
            stripped = stripped.replace(word, "");
        }
        if stripped.chars().any(char::is_alphanumeric) {
            cleaned.push_str(&rest[open..=open + close]);
        }
        rest = &rest[open + close + 1..];
    }
    cleaned.push_str(rest);
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    for dash in [" - ", " – ", " — "] {
        if let Some((artist, title)) = cleaned.split_once(dash)
            && !artist.trim().is_empty()
            && !title.trim().is_empty()
        {
            return (title.trim().to_string(), artist.trim().to_string());
        }
    }
    let artist = if uploader.trim().is_empty() {
        "Unknown artist"
    } else {
        uploader.trim()
    };
    (cleaned.trim().to_string(), artist.to_string())
}

/// Downloads the song into `folder`, tagged with `title`, `artist` and the
/// source as album. Returns where the file was saved.
pub async fn download(
    cx: &Ctx,
    preview: &Preview,
    title: &str,
    artist: &str,
    folder: &Path,
) -> Result<PathBuf> {
    let stem = format!("{:016x}", rand::random::<u64>());
    let mut command = command(&cx.tools.ytdlp);
    command
        .args(cx.tools.ytdlp_base())
        .args([
            "--no-playlist",
            "-f",
            "bestaudio/best",
            "-x",
            "--audio-format",
            "mp3",
            "--postprocessor-args",
            "ExtractAudio:-ar 44100 -ac 2",
            "--audio-quality",
            "0",
            "--retries",
            "2",
            "-o",
        ])
        .arg(cx.work_dir.join(format!("{stem}.%(ext)s")))
        .arg(&preview.url);
    run(cx, command).await?;
    let file = cx.work_dir.join(format!("{stem}.mp3"));
    if !file.is_file() {
        bail!("yt-dlp did not produce an MP3");
    }
    let cover = match &preview.thumbnail {
        Some(url) => fetch_cover(&cx.http, url).await,
        None => None,
    };
    let (path, tag_title, tag_artist, source, link) = (
        file.clone(),
        title.to_string(),
        artist.to_string(),
        preview.source.clone(),
        preview.url.clone(),
    );
    tokio::task::spawn_blocking(move || tag(&path, &tag_title, &tag_artist, &source, &link, cover))
        .await
        .map_err(|error| anyhow!("{error}"))??;
    tokio::fs::create_dir_all(folder).await?;
    let mut target = folder.join(sanitize(&format!("{artist} - {title}.mp3")));
    let mut copy = 2;
    while target.is_file() {
        target = folder.join(sanitize(&format!("{artist} - {title} ({copy}).mp3")));
        copy += 1;
    }
    if tokio::fs::rename(&file, &target).await.is_err() {
        tokio::fs::copy(&file, &target).await?;
        let _ = tokio::fs::remove_file(&file).await;
    }
    Ok(target)
}

/// The cover as a square JPEG. YouTube's widest picture comes first, then
/// the one the list showed.
async fn fetch_cover(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Some(rest) = url.split("i.ytimg.com/vi/").nth(1)
        && let Some(id) = rest.split('/').next()
    {
        candidates.push(format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"));
    }
    candidates.push(url.to_string());
    for candidate in candidates {
        let Some(bytes) = fetch_image(http, &candidate).await else {
            continue;
        };
        return tokio::task::spawn_blocking(move || square(bytes)).await.ok();
    }
    None
}

async fn fetch_image(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let bytes = http.get(url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
    let is_image = bytes.starts_with(&[0xFF, 0xD8]) || bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    is_image.then(|| bytes.to_vec())
}

/// Video thumbnails are wide; covers are square. The middle is kept.
fn square(bytes: Vec<u8>) -> Vec<u8> {
    let Ok(picture) = image::load_from_memory(&bytes) else {
        return bytes;
    };
    let (width, height) = (picture.width(), picture.height());
    if width == height {
        return bytes;
    }
    let side = width.min(height);
    let cropped = picture.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    let mut out = std::io::Cursor::new(Vec::new());
    match cropped.to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg) {
        Ok(()) => out.into_inner(),
        Err(_) => bytes,
    }
}

fn tag(path: &Path, title: &str, artist: &str, album: &str, link: &str, cover: Option<Vec<u8>>) -> Result<()> {
    let mut tagged = lofty::read_from_path(path)?;
    let kind = tagged.primary_tag_type();
    // A clean tag, so nothing from the upload disagrees with what Spotify
    // is told about the file.
    tagged.clear();
    tagged.insert_tag(Tag::new(kind));
    let tag = tagged.primary_tag_mut().context("no tag")?;
    tag.set_title(title.to_string());
    tag.set_artist(artist.to_string());
    tag.set_album(album.to_string());
    tag.set_comment(link.to_string());
    if let Some(bytes) = cover {
        let mime = if bytes.starts_with(&[0x89, b'P']) {
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

/// Songs imported with ID3v2.4 tags, rewritten as ID3v2.3 so Spotify shows
/// their covers too.
pub fn upgrade_tags(folder: &Path) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp3")) {
            continue;
        }
        let mut header = [0u8; 4];
        let v24 = std::fs::File::open(&path)
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut header))
            .is_ok_and(|()| header == *b"ID3\x04");
        if !v24 {
            continue;
        }
        let rewritten = lofty::read_from_path(&path).and_then(|tagged| {
            tagged.primary_tag().map_or(Ok(()), |tag| {
                tag.save_to_path(&path, WriteOptions::default().use_id3v23(true))
            })
        });
        if let Err(error) = rewritten {
            log::warn!("could not rewrite the tags of {}: {error}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_video_noise() {
        assert_eq!(
            clean_name("Clairo - Sofia (Official Video) [HD]", "Clairo"),
            ("Sofia".to_string(), "Clairo".to_string())
        );
        assert_eq!(
            clean_name("lying has to stop (feat. someone)", "Clairo"),
            ("lying has to stop (feat. someone)".to_string(), "Clairo".to_string())
        );
    }

    #[test]
    fn links_are_songs_or_lists() {
        assert_eq!(
            normalize_link("youtu.be/dQw4w9WgXcQ").as_deref(),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        assert_eq!(
            normalize_link("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=RDdQw4w9WgXcQ&start_radio=1").as_deref(),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        assert_eq!(
            normalize_link("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLabc").as_deref(),
            Some("https://www.youtube.com/playlist?list=PLabc")
        );
        assert!(normalize_link("https://soundcloud.com/a/b?in=a/sets/c").is_some_and(|url| !url.contains('?')));
        assert!(normalize_link("https://example.com").is_none());
    }
}
