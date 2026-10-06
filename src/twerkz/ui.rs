//! Menu entries, the import dialog and the job cards.

use std::path::PathBuf;
use std::time::Instant;

use egui::{Align2, CornerRadius, Frame, Margin, Stroke, Ui, vec2};

use super::{Event, Format, Request, Step};
use crate::app::App;
use crate::backend::Command;
use crate::theme::{self, Icon};
use crate::ui::widgets::{menu_item, menu_submenu};

#[derive(Clone, Debug)]
pub enum Action {
    Download {
        uri: String,
        name: String,
        format: Format,
    },
    Cancel(u64),
    Dismiss(u64),
    OpenFolder(PathBuf),
}

#[derive(Default)]
pub struct State {
    jobs: Vec<Job>,
    next_id: u64,
    last_folder: Option<PathBuf>,
    romaji: Option<Romaji>,
    romaji_on: bool,
}

#[derive(Clone, PartialEq)]
enum Kind {
    Download,
    Import,
}

struct Job {
    id: u64,
    kind: Kind,
    /// What the job is: "Downloading “Album”", "Adding to “Playlist”".
    heading: String,
    /// The song being worked on, or what is being set up.
    title: String,
    artist: String,
    cover: Option<String>,
    done: usize,
    saved: usize,
    failed: usize,
    total: usize,
    running: bool,
    error: bool,
    /// How it ended.
    summary: String,
    folder: Option<PathBuf>,
    ended: Option<Instant>,
}

impl Job {
    fn new(id: u64, kind: Kind, heading: String, title: &str) -> Self {
        Job {
            id,
            kind,
            heading,
            title: title.to_string(),
            artist: String::new(),
            cover: None,
            done: 0,
            saved: 0,
            failed: 0,
            total: 0,
            running: true,
            error: false,
            summary: String::new(),
            folder: None,
            ended: None,
        }
    }

    fn end(&mut self, error: bool, summary: String) {
        self.running = false;
        self.error = error;
        self.summary = summary;
        self.ended = Some(Instant::now());
    }
}

/// "Download" with a format submenu, for songs, albums and playlists.
pub fn download_menu(ui: &mut Ui, app: &mut App, uri: &str, name: &str) {
    let kind = crate::util::uri_kind(uri).unwrap_or("");
    if !matches!(kind, "track" | "album" | "playlist") {
        return;
    }
    let palette = app.palette;
    let label = if kind == "track" { "Download" } else { "Download as files" };
    menu_submenu(ui, &palette, Some(Icon::Download), label, |ui| {
        ui.set_min_width(96.0);
        ui.set_max_width(96.0);
        for format in Format::ALL {
            if menu_item(ui, &palette, Some(Icon::Music), format.label()) {
                app.actions.push(crate::model::Action::Twerkz(Action::Download {
                    uri: uri.to_string(),
                    name: name.to_string(),
                    format,
                }));
            }
        }
    });
}

pub fn apply(app: &mut App, action: Action) {
    match action {
        Action::Download { uri, name, format } => {
            let state = &mut app.twerkz;
            state.next_id += 1;
            let id = state.next_id;
            state.jobs.push(Job::new(
                id,
                Kind::Download,
                format!("Downloading “{name}” · {}", format.label()),
                "Choose a folder…",
            ));
            let mut dialog = rfd::AsyncFileDialog::new().set_title(format!("Download {name} to…"));
            if let Some(folder) = &state.last_folder {
                dialog = dialog.set_directory(folder);
            }
            let folder = dialog.pick_folder();
            app.backend.send(Command::Twerkz(Request::Download {
                id,
                uri,
                name,
                format,
                folder: Box::pin(folder),
            }));
        }
        Action::Cancel(id) => {
            app.backend.send(Command::Twerkz(Request::Cancel { id }));
            if let Some(job) = app.twerkz.jobs.iter_mut().find(|job| job.id == id) {
                job.title = "Cancelling…".to_string();
                job.artist.clear();
            }
        }
        Action::Dismiss(id) => app.twerkz.jobs.retain(|job| job.id != id),
        Action::OpenFolder(folder) => {
            if let Err(error) = crate::opener::open(&folder) {
                app.toast_error(format!("Couldn't open the folder: {error}"));
            }
        }
    }
}

pub fn on_event(app: &mut App, event: Event) {
    let state = &mut app.twerkz;
    let id = match &event {
        Event::Dismissed { id }
        | Event::Status { id, .. }
        | Event::Progress { id, .. }
        | Event::Finished { id, .. }
        | Event::Failed { id, .. } => *id,
        Event::Romanized { uri, result } => {
            on_romanized(app, uri.clone(), result.clone());
            return;
        }
    };
    let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) else {
        return;
    };
    match event {
        Event::Dismissed { .. } => state.jobs.retain(|job| job.id != id),
        Event::Status { text, .. } => {
            job.title = text;
            job.artist.clear();
        }
        Event::Progress {
            done,
            failed,
            total,
            title,
            artist,
            cover,
            ..
        } => {
            job.done = done;
            job.failed = failed;
            job.total = total;
            if !title.is_empty() {
                job.title = title;
                job.artist = artist;
            } else if job.done == 0 && job.title.ends_with('…') {
                job.title = "Starting…".to_string();
            }
            if cover.is_some() {
                job.cover = cover;
            }
        }
        Event::Finished {
            saved,
            skipped,
            failed,
            folder,
            ..
        } => {
            job.saved = saved;
            job.done = job.total;
            let mut parts = vec![if failed.is_empty() {
                "Downloaded".to_string()
            } else {
                "Downloaded with errors".to_string()
            }];
            parts.push(format!("{saved} saved"));
            if skipped > 0 {
                parts.push(format!("{skipped} already there"));
            }
            if !failed.is_empty() {
                parts.push(format!("{} failed", failed.len()));
            }
            job.end(!failed.is_empty(), parts.join(" · "));
            state.last_folder = Some(if job.total > 1 {
                folder.parent().map(PathBuf::from).unwrap_or(folder.clone())
            } else {
                folder.clone()
            });
            job.folder = Some(folder);
        }
        Event::Failed { message, .. } => {
            if message == "cancelled" {
                job.end(false, "Cancelled".to_string());
            } else {
                job.end(true, message);
            }
        }
    }
}

/// Romanized lyrics for the song on screen.
struct Romaji {
    uri: String,
    line_count: usize,
    script: Option<super::romanize::Script>,
    asked: bool,
    lines: Option<Result<Vec<String>, String>>,
}

/// The romanized lines for `uri`, when the toggle is on and they are ready.
/// Asks for them the first time they are wanted.
pub fn romaji_for(app: &mut App, uri: &str, lyrics: &crate::lyrics::Lyrics) -> Option<Vec<String>> {
    let fresh = app
        .twerkz
        .romaji
        .as_ref()
        .is_none_or(|held| held.uri != uri || held.line_count != lyrics.lines.len());
    if fresh {
        app.twerkz.romaji = Some(Romaji {
            uri: uri.to_string(),
            line_count: lyrics.lines.len(),
            script: super::romanize::detect(lyrics.lines.iter().map(|line| line.text.as_str())),
            asked: false,
            lines: None,
        });
    }
    let held = app.twerkz.romaji.as_mut()?;
    held.script?;
    if !app.twerkz.romaji_on {
        return None;
    }
    if !held.asked {
        held.asked = true;
        app.backend.send(Command::Twerkz(Request::Romanize {
            uri: uri.to_string(),
            lines: lyrics.lines.clone(),
        }));
    }
    match &held.lines {
        Some(Ok(lines)) if lines.len() == lyrics.lines.len() => Some(lines.clone()),
        _ => None,
    }
}

/// Line `index` in Latin letters when romaji is shown, else as written.
pub fn romaji_text<'a>(romaji: Option<&'a Vec<String>>, index: usize, original: &'a str) -> &'a str {
    romaji
        .and_then(|lines| lines.get(index))
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .unwrap_or(original)
}

/// The romaji toggle: shown while the pointer is over the lyrics, only
/// for Japanese, Korean or Chinese words.
pub fn romaji_button(app: &mut App, ui: &mut Ui, hovered: bool) {
    let Some(script) = app.twerkz.romaji.as_ref().and_then(|held| held.script) else {
        return;
    };
    let on = app.twerkz.romaji_on;
    if !hovered {
        return;
    }
    let (glyph, name) = match script {
        super::romanize::Script::Japanese => ("あ", "romaji"),
        super::romanize::Script::Korean => ("가", "romaja"),
        super::romanize::Script::Chinese => ("拼", "pinyin"),
    };
    let palette = app.palette;
    let loading = on
        && app
            .twerkz
            .romaji
            .as_ref()
            .is_some_and(|held| held.lines.is_none());
    let response = theme::soft_button(ui, &palette, None, glyph, on)
        .on_hover_text(if on { format!("Hide {name}") } else { format!("Show {name}") });
    if loading {
        ui.add(egui::Spinner::new().size(12.0).color(palette.secondary));
    }
    if response.clicked() {
        app.twerkz.romaji_on = !on;
    }
}

fn on_romanized(app: &mut App, uri: String, result: Result<Vec<String>, String>) {
    if let Some(held) = app.twerkz.romaji.as_mut()
        && held.uri == uri
    {
        if let Err(error) = &result {
            log::warn!("romanization failed: {error}");
        }
        held.lines = Some(result);
    }
}
