//! Extras on top of upstream: more lyrics providers, song downloads,
//! YouTube and SoundCloud imports, and a user emoji font. Kept in one
//! folder so upstream merges stay small.

pub mod download;
pub mod jobs;
pub mod lyrics;
pub mod romanize;
pub mod tools;
pub mod ui;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

pub use download::Format;

/// A folder picker, built on the UI thread and awaited on the runtime.
pub type FolderPick = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>> + Send>>;
pub type FilePick = FolderPick;

/// What the app asks the backend for.
pub enum Request {
    Download {
        id: u64,
        uri: String,
        name: String,
        format: Format,
        folder: FolderPick,
    },
    Cancel {
        id: u64,
    },
    Romanize {
        uri: String,
        lines: Vec<crate::lyrics::Line>,
    },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Download { id, uri, format, .. } => {
                write!(f, "Download {{ id: {id}, uri: {uri}, format: {format:?} }}")
            }
            Request::Cancel { id } => write!(f, "Cancel {{ id: {id} }}"),
            Request::Romanize { uri, .. } => write!(f, "Romanize {{ uri: {uri} }}"),
        }
    }
}

/// What the backend tells the app about a job.
#[derive(Clone, Debug)]
pub enum Event {
    /// The folder picker was closed without a choice.
    Dismissed { id: u64 },
    Status { id: u64, text: String },
    /// `title` is the song now downloading, empty when unchanged.
    Progress {
        id: u64,
        done: usize,
        failed: usize,
        total: usize,
        title: String,
        artist: String,
        cover: Option<String>,
    },
    Finished {
        id: u64,
        saved: usize,
        skipped: usize,
        failed: Vec<String>,
        folder: PathBuf,
    },
    Failed { id: u64, message: String },
    /// The lyrics of `uri` in Latin letters, one per line.
    Romanized {
        uri: String,
        result: Result<Vec<String>, String>,
    },
}

/// Lowercased words without punctuation.
fn normalize(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut space = true;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    out
}

/// 1.0 for the same words, falling towards 0.0 with edit distance.
pub fn similarity(left: &str, right: &str) -> f64 {
    let a = normalize(left);
    let b = normalize(right);
    if a == b {
        return 1.0;
    }
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    1.0 - previous[b.len()] as f64 / longest as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_ignores_case_and_punctuation() {
        assert_eq!(similarity("Hello, World!", "hello world"), 1.0);
        assert!(similarity("Hello", "Goodbye") < 0.5);
    }
}
