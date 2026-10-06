//! More lyrics providers behind Spotify's own: Musixmatch, NetEase, LRCLIB
//! and Genius. The first three are asked at once; a synced answer wins in
//! that order, then the first plain one. Genius only has plain lyrics, so it
//! is asked last and only when nobody else knew the song.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use sha1::{Digest, Sha1};

use super::similarity;
use crate::lyrics::{Line, Lyrics, Query, clean_artist, clean_title, parse_lrc};

const MXM: &str = "https://apic-appmobile.musixmatch.com/ws/1.1";
const MXM_APP: &str = "mac-ios-v2.0";

const NETEASE: &str = "https://music.163.com/api";
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(10);

static MXM_TOKEN: Mutex<Option<String>> = Mutex::new(None);
static MXM_FAILED: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// NetEase's own romanized lines for songs whose lyrics came from NetEase,
/// by track URI. Human-made, so they win over computed romaji.
static NETEASE_ROMAJI: std::sync::LazyLock<Mutex<std::collections::HashMap<String, Vec<Line>>>> =
    std::sync::LazyLock::new(Default::default);

/// NetEase's romanized lines for `uri`, matched to `lines` by timestamp,
/// when they cover nearly every line.
pub fn netease_romaji(uri: &str, lines: &[Line]) -> Option<Vec<String>> {
    let held = NETEASE_ROMAJI.lock().ok()?;
    let romaji = held.get(uri)?;
    let by_time: std::collections::HashMap<u32, &str> = romaji
        .iter()
        .filter_map(|line| Some((line.at_ms?, line.text.as_str())))
        .collect();
    let mut matched = 0;
    let out: Vec<String> = lines
        .iter()
        .map(|line| match line.at_ms.and_then(|at| by_time.get(&at)) {
            Some(text) => {
                matched += 1;
                text.to_string()
            }
            None => String::new(),
        })
        .collect();
    let wanted = lines.iter().filter(|line| !line.text.trim().is_empty()).count();
    (wanted > 0 && matched * 10 >= wanted * 8).then_some(out)
}

/// How alike two names are, also across scripts: the best of comparing
/// them as written and by their romanized forms.
fn name_similarity(left: &str, right: &str) -> f64 {
    let left_latin = crate::twerkz::romanize::quick(left);
    let right_latin = crate::twerkz::romanize::quick(right);
    let mut best = similarity(left, right);
    for (a, b) in [
        (left_latin.as_deref(), Some(right)),
        (Some(left), right_latin.as_deref()),
        (left_latin.as_deref(), right_latin.as_deref()),
    ] {
        if let (Some(a), Some(b)) = (a, b) {
            best = best.max(similarity(a, b));
        }
    }
    best
}

/// Whether any of `names` is the wanted artist: alike enough, or one name
/// holding the other ("A & B" for "A").
fn artist_matches(names: &[&str], wanted: &str) -> bool {
    let fold = |text: &str| -> String {
        text.chars()
            .flat_map(char::to_lowercase)
            .filter(|c| c.is_alphanumeric())
            .collect()
    };
    let wanted_folded = fold(wanted);
    names.iter().any(|name| {
        let folded = fold(name);
        name_similarity(name, wanted) >= 0.7
            || (folded.chars().count() >= 3
                && wanted_folded.chars().count() >= 3
                && (folded.contains(&wanted_folded) || wanted_folded.contains(&folded)))
    })
}

pub async fn fetch(
    http: &reqwest::Client,
    cache_dir: &Path,
    uri: &str,
    query: &Query,
) -> Result<Option<Lyrics>, String> {
    let path = cache_dir.join(format!("twerkz-{}.json", cache_key(uri, query)));
    if let Some(cached) = crate::lyrics::cached(&path) {
        return Ok(cached);
    }
    let spotify_id = uri.strip_prefix("spotify:track:");
    // Names in Japanese are compared by their romaji too.
    crate::twerkz::romanize::prepare(http, &format!("{} {}", query.title, query.artist)).await;
    let (mxm, netease, lrclib) = tokio::join!(
        musixmatch(http, cache_dir, spotify_id, query),
        netease(http, query),
        crate::lyrics::fetch(http, cache_dir, query),
    );
    let lrclib_failed = lrclib.is_err();
    let lrclib = lrclib.unwrap_or_else(|error| {
        log::debug!("lrclib: {error:#}");
        None
    });
    let (netease, netease_romaji) = match netease {
        Some((lyrics, romaji)) => (Some(lyrics), romaji),
        None => (None, None),
    };
    let from_netease = netease.clone();
    let found = match pick([mxm, netease, lrclib]) {
        Some(found) => Some(found),
        None => genius(http, query).await,
    };
    if let Some(romaji) = netease_romaji
        && found.is_some()
        && found == from_netease
        && let Ok(mut held) = NETEASE_ROMAJI.lock()
    {
        held.insert(uri.to_string(), romaji);
    }
    if found.is_none() && lrclib_failed {
        return Err("cannot reach the lyrics providers".to_string());
    }
    crate::lyrics::store(&path, &found);
    Ok(found)
}

fn pick(ranked: [Option<Lyrics>; 3]) -> Option<Lyrics> {
    if let Some(index) = ranked
        .iter()
        .position(|found| found.as_ref().is_some_and(|lyrics| lyrics.synced))
    {
        return ranked.into_iter().nth(index).flatten();
    }
    ranked.into_iter().flatten().next()
}

fn plain(text: &str) -> Option<Lyrics> {
    let lines: Vec<Line> = text
        .lines()
        .map(str::trim_end)
        .map(|text| Line {
            at_ms: None,
            text: text.to_string(),
        })
        .collect();
    let first = lines.iter().position(|line| !line.text.trim().is_empty())?;
    let last = lines
        .iter()
        .rposition(|line| !line.text.trim().is_empty())?;
    Some(Lyrics {
        lines: lines[first..=last].to_vec(),
        synced: false,
        instrumental: false,
    })
}

fn instrumental() -> Lyrics {
    Lyrics {
        instrumental: true,
        ..Lyrics::default()
    }
}

// ---- Musixmatch -------------------------------------------------------------

fn mxm_request(http: &reqwest::Client, url: String) -> reqwest::RequestBuilder {
    http.get(url)
        .timeout(TIMEOUT)
        .header(
            "User-Agent",
            "Musixmatch/2025120901 CFNetwork/3860.300.31 Darwin/25.2.0",
        )
        .header("X-Cookie", "x-mxm-token-guid=")
        .header("x-mxm-app-version", "10.1.1")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept", "application/json")
}

fn usable_token(body: &Value) -> Option<String> {
    body.pointer("/message/body/user_token")
        .and_then(Value::as_str)
        .filter(|token| {
            !token.is_empty()
                && !token.starts_with("UpgradeOnly")
                && token.chars().any(|c| c != '0')
        })
        .map(str::to_string)
}

/// A user token from the mobile endpoint, else the desktop one. Musixmatch
/// answers with a captcha when asked too often, so a failure waits ten
/// minutes before the next try and a working token is kept on disk.
async fn mxm_token(http: &reqwest::Client, cache_dir: &Path, refresh: bool) -> Option<String> {
    let file = cache_dir.join("musixmatch-token.txt");
    if !refresh {
        if let Some(token) = MXM_TOKEN.lock().ok().and_then(|held| held.clone()) {
            return Some(token);
        }
        if let Ok(token) = std::fs::read_to_string(&file)
            && !token.trim().is_empty()
        {
            let token = token.trim().to_string();
            if let Ok(mut held) = MXM_TOKEN.lock() {
                *held = Some(token.clone());
            }
            return Some(token);
        }
    }
    if MXM_FAILED
        .lock()
        .ok()
        .and_then(|failed| *failed)
        .is_some_and(|at| at.elapsed() < Duration::from_secs(600))
    {
        return None;
    }
    let mobile = async {
        let body: Value = mxm_request(http, format!("{MXM}/token.get?app_id={MXM_APP}"))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        usable_token(&body)
    };
    let desktop = async {
        let body: Value = http
            .get("https://apic-desktop.musixmatch.com/ws/1.1/token.get?app_id=web-desktop-app-v1.0&user_language=en")
            .timeout(TIMEOUT)
            .header("Cookie", "AWSELBCORS=0; AWSELB=0")
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        usable_token(&body)
    };
    let token = match mobile.await {
        Some(token) => Some(token),
        None => desktop.await,
    };
    match &token {
        Some(token) => {
            let _ = std::fs::create_dir_all(cache_dir);
            let _ = std::fs::write(&file, token);
        }
        None => {
            if let Ok(mut failed) = MXM_FAILED.lock() {
                *failed = Some(std::time::Instant::now());
            }
        }
    }
    if let Ok(mut held) = MXM_TOKEN.lock() {
        *held = token.clone();
    }
    token
}

async fn mxm_query(
    http: &reqwest::Client,
    token: &str,
    spotify_id: Option<&str>,
    query: &Query,
) -> Option<Value> {
    let seconds = (query.duration_ms / 1000).to_string();
    let mut params = vec![
        ("format", "json".to_string()),
        ("namespace", "lyrics_richsynched".to_string()),
        ("subtitle_format", "mxm".to_string()),
        ("app_id", MXM_APP.to_string()),
        ("q_album", query.album.clone()),
        ("q_artist", query.artist.clone()),
        ("q_artists", query.artist.clone()),
        ("q_track", query.title.clone()),
        ("q_duration", seconds.clone()),
        ("f_subtitle_length", seconds),
        ("usertoken", token.to_string()),
    ];
    if let Some(id) = spotify_id {
        params.push(("track_spotify_id", format!("spotify:track:{id}")));
    }
    let query_string = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    mxm_request(http, format!("{MXM}/macro.subtitles.get?{query_string}"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()
}

async fn musixmatch(
    http: &reqwest::Client,
    cache_dir: &Path,
    spotify_id: Option<&str>,
    query: &Query,
) -> Option<Lyrics> {
    let token = mxm_token(http, cache_dir, false).await?;
    let mut body = mxm_query(http, &token, spotify_id, query).await?;
    let status = body
        .pointer("/message/header/status_code")
        .and_then(Value::as_u64);
    if matches!(status, Some(401 | 402)) {
        let _ = std::fs::remove_file(cache_dir.join("musixmatch-token.txt"));
        let token = mxm_token(http, cache_dir, true).await?;
        body = mxm_query(http, &token, spotify_id, query).await?;
    }
    let calls = body.pointer("/message/body/macro_calls")?;
    let matcher = calls.get("matcher.track.get")?;
    if matcher
        .pointer("/message/header/status_code")
        .and_then(Value::as_u64)
        != Some(200)
    {
        return None;
    }
    let track = matcher.pointer("/message/body/track")?;
    let flag = |key: &str| track.get(key).and_then(Value::as_u64) == Some(1);
    // A fuzzy fallback match echoes the title but not the Spotify id, and
    // carries another song's words.
    match spotify_id {
        Some(id) => {
            if track.get("track_spotify_id").and_then(Value::as_str) != Some(id) {
                return None;
            }
        }
        None => {
            let length = track
                .get("track_length")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if length > 0 && (length as i64 - i64::from(query.duration_ms / 1000)).abs() > 15 {
                return None;
            }
        }
    }
    if flag("instrumental") {
        return Some(instrumental());
    }
    if flag("has_subtitles")
        && let Some(raw) = calls
            .pointer("/track.subtitles.get/message/body/subtitle_list/0/subtitle/subtitle_body")
            .and_then(Value::as_str)
        && let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(raw)
    {
        let lines: Vec<Line> = rows
            .iter()
            .filter_map(|row| {
                let total = row.pointer("/time/total")?.as_f64()?;
                let text = row.get("text").and_then(Value::as_str).unwrap_or("").trim();
                Some(Line {
                    at_ms: Some((total * 1000.0).round() as u32),
                    text: if text.is_empty() {
                        "\u{266a}".to_string()
                    } else {
                        text.to_string()
                    },
                })
            })
            .collect();
        if !lines.is_empty() {
            return Some(Lyrics {
                lines,
                synced: true,
                instrumental: false,
            });
        }
    }
    let lyrics = calls.pointer("/track.lyrics.get/message/body/lyrics")?;
    if lyrics.get("restricted").and_then(Value::as_u64) == Some(1) {
        return None;
    }
    let body = lyrics.get("lyrics_body").and_then(Value::as_str)?;
    let kept: String = body
        .lines()
        .take_while(|line| !line.trim_start().starts_with("*******"))
        .collect::<Vec<_>>()
        .join("\n");
    plain(&kept)
}

// ---- NetEase ----------------------------------------------------------------

fn netease_request(http: &reqwest::Client, url: String) -> reqwest::RequestBuilder {
    http.get(url)
        .timeout(TIMEOUT)
        .header("User-Agent", BROWSER_UA)
        .header("Referer", "https://music.163.com")
        .header("Origin", "https://music.163.com")
}

/// NetEase opens many lyrics with credit lines that carry timestamps.
fn is_credit(text: &str) -> bool {
    const CREDITS: &[&str] = &[
        "作词",
        "作曲",
        "编曲",
        "制作人",
        "词",
        "曲",
        "混音",
        "母带",
        "和声",
        "录音",
        "监制",
        "出品",
        "lyrics",
        "lyricist",
        "composer",
        "composed",
        "arranger",
        "arranged",
        "producer",
        "produced",
        "written",
    ];
    let lower = text.trim().to_lowercase();
    let Some((head, _)) = lower.split_once([':', '：']) else {
        return false;
    };
    let head = head.trim();
    CREDITS
        .iter()
        .any(|credit| head == *credit || head.starts_with(credit))
        && head.chars().count() <= 14
}

async fn netease_search(http: &reqwest::Client, search: &str) -> Option<Value> {
    netease_request(
        http,
        format!(
            "{NETEASE}/search/get?s={}&type=1&offset=0&limit=10",
            urlencoding::encode(search.trim())
        ),
    )
    .send()
    .await
    .ok()?
    .json()
    .await
    .ok()
}

/// Search results that are this song, best first. The title must match
/// and the length be within 8 s; the artist must match too, unless the
/// title is exact and the length within 3 s (a name in another script).
fn netease_candidates(body: &Value, title: &str, artist: &str, query: &Query) -> Vec<(f64, u64)> {
    let Some(songs) = body.pointer("/result/songs").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut scored: Vec<(f64, u64)> = songs
        .iter()
        .filter_map(|song| {
            let id = song.get("id")?.as_u64()?;
            let name = song.get("name")?.as_str()?;
            let artists: Vec<&str> = song
                .get("artists")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(|a| a.get("name")?.as_str()).collect())
                .unwrap_or_default();
            let title_score = name_similarity(&clean_title(name), title);
            if title_score < 0.75 {
                return None;
            }
            let duration = song.get("duration").and_then(Value::as_u64).unwrap_or(0);
            let drift = if duration > 0 && query.duration_ms > 0 {
                (duration as f64 - f64::from(query.duration_ms)).abs() / 1000.0
            } else {
                4.0
            };
            if drift > 8.0 {
                return None;
            }
            let by_artist = artist_matches(&artists, artist);
            if !by_artist && !(title_score >= 0.95 && drift <= 3.0) {
                return None;
            }
            let score = title_score * 0.5
                + if by_artist { 0.3 } else { 0.0 }
                + (1.0 - drift / 8.0) * 0.2;
            Some((score, id))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
}

/// NetEase's lyrics and, when it has them, its own romanized lines.
async fn netease(http: &reqwest::Client, query: &Query) -> Option<(Lyrics, Option<Vec<Line>>)> {
    let title = clean_title(&query.title);
    let artist = clean_artist(&query.artist);
    if title.is_empty() {
        return None;
    }
    let mut scored = match netease_search(http, &format!("{title} {artist}")).await {
        Some(body) => netease_candidates(&body, &title, &artist, query),
        None => Vec::new(),
    };
    // NetEase files many songs under another script than Spotify does, so
    // a search that found nothing tries the romanized names.
    if scored.is_empty()
        && let Some(title_latin) = crate::twerkz::romanize::quick(&title)
    {
        let artist_latin = crate::twerkz::romanize::quick(&artist).unwrap_or_else(|| artist.clone());
        if let Some(body) = netease_search(http, &format!("{title_latin} {artist_latin}")).await {
            scored = netease_candidates(&body, &title, &artist, query);
        }
    }
    for (_, id) in scored.into_iter().take(4) {
        let Some(data) = netease_request(
            http,
            format!("{NETEASE}/song/lyric?id={id}&lv=1&kv=1&tv=-1&rv=-1"),
        )
        .send()
        .await
        .ok() else {
            continue;
        };
        let Ok(data) = data.json::<Value>().await else {
            continue;
        };
        if data.get("nolyric").and_then(Value::as_bool) == Some(true)
            || data.get("pureMusic").and_then(Value::as_bool) == Some(true)
        {
            return Some((instrumental(), None));
        }
        let Some(raw) = data.pointer("/lrc/lyric").and_then(Value::as_str) else {
            continue;
        };
        let lines: Vec<Line> = parse_lrc(raw)
            .into_iter()
            .filter(|line| !is_credit(&line.text))
            .collect();
        if lines.iter().any(|line| !line.text.trim().is_empty()) {
            let romanized = data
                .pointer("/romalrc/lyric")
                .and_then(Value::as_str)
                .filter(|roma| !roma.trim().is_empty() && *roma != raw)
                .map(|roma| {
                    parse_lrc(roma)
                        .into_iter()
                        .filter(|line| !is_credit(&line.text))
                        .collect::<Vec<_>>()
                })
                .filter(|roma| !roma.is_empty());
            return Some((
                Lyrics {
                    lines,
                    synced: true,
                    instrumental: false,
                },
                romanized,
            ));
        }
        if !raw.contains('[')
            && let Some(found) = plain(raw)
        {
            return Some((found, None));
        }
    }
    None
}

// ---- Genius -----------------------------------------------------------------

async fn genius(http: &reqwest::Client, query: &Query) -> Option<Lyrics> {
    let title = clean_title(&query.title);
    let artist = clean_artist(&query.artist);
    if title.is_empty() {
        return None;
    }
    let search = format!("{title} {artist}");
    let body: Value = http
        .get(format!(
            "https://genius.com/api/search/song?q={}",
            urlencoding::encode(search.trim())
        ))
        .timeout(TIMEOUT)
        .header("User-Agent", BROWSER_UA)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let hits = body.pointer("/response/sections/0/hits")?.as_array()?;
    let path = hits.iter().find_map(|hit| {
        let result = hit.get("result")?;
        let name = result.get("title")?.as_str()?;
        let by = result
            .pointer("/primary_artist/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        (name_similarity(&clean_title(name), &title) >= 0.75 && artist_matches(&[by], &artist))
            .then(|| result.get("path")?.as_str().map(str::to_string))
            .flatten()
    })?;
    let html = http
        .get(format!("https://genius.com{path}"))
        .timeout(TIMEOUT)
        .header("User-Agent", BROWSER_UA)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    genius_lines(&html)
}

fn genius_lines(html: &str) -> Option<Lyrics> {
    let mut text = String::new();
    let mut rest = html;
    while let Some(at) = rest.find("data-lyrics-container=\"true\"") {
        let after = &rest[at..];
        let open_end = after.find('>')? + 1;
        let (inner, used) = div_inner(&after[open_end..]);
        text.push_str(&html_to_text(inner));
        text.push('\n');
        rest = &after[open_end + used..];
    }
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !(line.contains('[') && line.contains(']')))
        .map(str::to_string)
        .collect();
    if let Some(first) = lines.first_mut()
        && let Some(cut) = first.find("Lyrics")
        && first[..cut].contains("Contributor")
    {
        *first = first[cut + "Lyrics".len()..].trim().to_string();
    }
    if let Some(last) = lines.last_mut() {
        let trimmed = last
            .trim_end_matches("Embed")
            .trim_end_matches(|c: char| c.is_ascii_digit());
        *last = trimmed.trim().to_string();
    }
    let joined = lines
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    plain(&joined)
}

/// The inside of a div whose opening tag ends just before `html`, and how
/// many bytes it took including the closing tag.
fn div_inner(html: &str) -> (&str, usize) {
    let mut depth = 1usize;
    let mut index = 0;
    while index < html.len() {
        let rest = &html[index..];
        if rest.starts_with("<div") {
            depth += 1;
        } else if rest.starts_with("</div") {
            depth -= 1;
            if depth == 0 {
                let close = rest.find('>').map_or(rest.len(), |end| end + 1);
                return (&html[..index], index + close);
            }
        }
        index += rest.chars().next().map_or(1, char::len_utf8);
    }
    (html, html.len())
}

/// Lyrics text from Genius markup: line breaks kept, annotation chrome
/// (`data-exclude-from-selection`) dropped, other tags removed.
fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tag_end = rest[lt..].find('>').map_or(rest.len(), |end| lt + end + 1);
        let tag = &rest[lt..tag_end];
        if tag.starts_with("<br") {
            out.push('\n');
        }
        if tag.starts_with("<div") && tag.contains("data-exclude-from-selection") {
            let (_, used) = div_inner(&rest[tag_end..]);
            rest = &rest[tag_end + used..];
            continue;
        }
        rest = &rest[tag_end..];
    }
    out.push_str(rest);
    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn cache_key(uri: &str, query: &Query) -> String {
    let digest = Sha1::digest(
        format!(
            "{uri}|{}|{}|{}|{}",
            query.artist, query.title, query.album, query.duration_ms
        )
        .as_bytes(),
    );
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_wins_over_an_earlier_plain_answer() {
        let plain_one = plain("a\nb");
        let synced = Some(Lyrics {
            lines: vec![Line {
                at_ms: Some(0),
                text: "x".into(),
            }],
            synced: true,
            instrumental: false,
        });
        assert!(pick([plain_one, None, synced]).is_some_and(|found| found.synced));
    }

    #[test]
    fn genius_markup_becomes_lines() {
        let html = r#"<div data-lyrics-container="true" class="x"><div data-exclude-from-selection="true">3 Contributors</div>[Verse 1]<br/>Hello &amp; you<br><i>there</i><div>inner</div></div>"#;
        let found = genius_lines(html).unwrap();
        let texts: Vec<&str> = found.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["Hello & you", "thereinner"]);
    }

    #[test]
    fn netease_credits_are_dropped() {
        assert!(is_credit("作词 : 某人"));
        assert!(is_credit("Composer: Someone"));
        assert!(!is_credit("I said: hello there my friend"));
    }
}

#[cfg(test)]
mod live {
    use super::*;

    fn song() -> Query {
        Query {
            artist: "The Weeknd".into(),
            title: "Blinding Lights".into(),
            album: "After Hours".into(),
            duration_ms: 200_040,
        }
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn each_provider_answers() {
        let http = reqwest::Client::new();
        let q = song();
        let mxm = musixmatch(
            &http,
            &std::env::temp_dir(),
            Some("0VjIjW4GlUZAMYd2vXMi3b"),
            &q,
        )
        .await;
        let ne = netease(&http, &q).await;
        let ge = genius(&http, &q).await;
        let show = |name: &str, found: &Option<Lyrics>| {
            eprintln!(
                "{name}: {:?}",
                found.as_ref().map(|l| (
                    l.synced,
                    l.lines.len(),
                    l.lines
                        .iter()
                        .find(|x| !x.text.is_empty())
                        .map(|x| x.text.clone())
                ))
            )
        };
        show("musixmatch", &mxm);
        show("netease", &ne);
        show("genius", &ge);
    }
}
