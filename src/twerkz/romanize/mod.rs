//! Lyrics in Latin letters: romaji for Japanese, romaja for Korean, pinyin
//! for Chinese. The language is told from the script the lyrics use.

mod chinese;
mod japanese;
mod korean;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

pub use japanese::kana_to_romaji;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Japanese,
    Korean,
    Chinese,
}

const IPADIC_URL: &str =
    "https://github.com/lindera/lindera/releases/download/v6.2.0/lindera-ipadic-6.2.0.zip";

/// The script of `lines`, or `None` for lyrics with no Japanese, Korean or
/// Chinese in them. Any Hangul is Korean; text that is nearly all Han with
/// no kana is Chinese; the rest is Japanese.
pub fn detect<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<Script> {
    let (mut kana, mut han, mut hangul) = (0usize, 0usize, 0usize);
    for c in lines.into_iter().flat_map(str::chars) {
        match c {
            '\u{3040}'..='\u{30FF}' | '\u{FF66}'..='\u{FF9F}' => kana += 1,
            '\u{AC00}'..='\u{D7A3}' | '\u{3131}'..='\u{3163}' => hangul += 1,
            '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' => han += 1,
            _ => {}
        }
    }
    let total = kana + han + hangul;
    if total == 0 {
        return None;
    }
    if hangul > 0 {
        return Some(Script::Korean);
    }
    // The ratio the original detector uses at its default threshold of 1%.
    let kana_share = kana as f64 / total as f64;
    let han_share = han as f64 / total as f64;
    if (kana_share - han_share + 1.0) / 2.0 * 100.0 >= 1.0 {
        Some(Script::Japanese)
    } else {
        Some(Script::Chinese)
    }
}

/// Whether `text` has anything to romanize.
pub fn has_cjk(text: &str) -> bool {
    detect([text]).is_some()
}

static JAPANESE: Mutex<Option<Arc<japanese::Japanese>>> = Mutex::new(None);

/// One setup at a time, so two lyrics loads never fetch the dictionary twice.
static SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Where the Japanese dictionary is kept.
fn root() -> PathBuf {
    super::jobs::root(&crate::paths::AppDirs::discover()).join("romanize")
}

/// The Japanese reader, fetching its dictionary on first use.
async fn japanese(http: &reqwest::Client) -> Result<Arc<japanese::Japanese>> {
    let _setup = SETUP.lock().await;
    if let Some(reader) = JAPANESE.lock().ok().and_then(|held| held.clone()) {
        return Ok(reader);
    }
    let root = root();
    let dir = root.join("ipadic");
    if !dir.join("metadata.json").is_file() {
        tokio::fs::create_dir_all(&root).await?;
        let archive = root.join("ipadic.zip");
        super::tools::download(http, IPADIC_URL, &archive).await?;
        let (archive_path, dir_path) = (archive.clone(), dir.clone());
        tokio::task::spawn_blocking(move || super::tools::unzip_folder(&archive_path, "lindera-ipadic/", &dir_path))
            .await??;
        let _ = tokio::fs::remove_file(&archive).await;
    }
    let reader = tokio::task::spawn_blocking(move || japanese::Japanese::load(&dir))
        .await
        .context("dictionary loader stopped")??;
    let reader = Arc::new(reader);
    if let Ok(mut held) = JAPANESE.lock() {
        *held = Some(reader.clone());
    }
    Ok(reader)
}

/// `lines` in Latin letters, one for one.
pub async fn romanize(
    http: &reqwest::Client,
    script: Script,
    lines: Vec<String>,
) -> Result<Vec<String>> {
    match script {
        Script::Korean => Ok(lines.iter().map(|line| korean::romanize(line)).collect()),
        Script::Chinese => Ok(lines.iter().map(|line| chinese::romanize(line)).collect()),
        Script::Japanese => {
            let reader = japanese(http).await?;
            tokio::task::spawn_blocking(move || {
                lines.iter().map(|line| reader.romanize(line)).collect()
            })
            .await
            .context("romanizer stopped")
        }
    }
}

/// A short romanized form for matching names across scripts, or `None`
/// when `text` has nothing to romanize or Japanese is not ready yet.
pub fn quick(text: &str) -> Option<String> {
    match detect([text])? {
        Script::Korean => Some(korean::romanize(text)),
        Script::Chinese => Some(chinese::romanize(text)),
        Script::Japanese => {
            let reader = JAPANESE.lock().ok()?.clone()?;
            Some(reader.romanize(text))
        }
    }
}

/// Loads the Japanese reader when `text` needs it, so [`quick`] can answer.
pub async fn prepare(http: &reqwest::Client, text: &str) {
    if detect([text]) == Some(Script::Japanese)
        && let Err(error) = japanese(http).await
    {
        log::warn!("japanese dictionary unavailable: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_told_from_the_letters() {
        assert_eq!(detect(["Hello there"]), None);
        assert_eq!(detect(["君の名は"]), Some(Script::Japanese));
        assert_eq!(detect(["사랑해"]), Some(Script::Korean));
        assert_eq!(detect(["我爱你"]), Some(Script::Chinese));
    }
}

#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore = "network, downloads the dictionary"]
    async fn japanese_lines_read_like_the_original() {
        let http = reqwest::Client::new();
        let lines = [
            "今日は君と会えた",
            "東京の夜に駆ける",
            "私は学校へ行きます",
            "沈むように溶けてゆくように",
            "ファンタジーなパーティー",
        ]
        .map(String::from)
        .to_vec();
        let out = romanize(&http, Script::Japanese, lines.clone()).await.unwrap();
        for (line, romaji) in lines.iter().zip(&out) {
            eprintln!("{line} -> {romaji}");
        }
    }
}
