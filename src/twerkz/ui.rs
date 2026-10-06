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

/// The job cards, bottom left above the player bar: the song being worked
/// on with its cover, how far along the job is, and a way to stop it.
pub fn panel(app: &mut App, ctx: &egui::Context) {
    app.twerkz.jobs.retain(|job| {
        job.ended
            .is_none_or(|ended| job.error || ended.elapsed().as_secs() < 15)
    });
    if app.twerkz.jobs.is_empty() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let palette = app.palette;
    let art = app.backend.art().clone();
    let mut actions = Vec::new();
    egui::Area::new(egui::Id::new("twerkz-downloads"))
        .anchor(
            Align2::LEFT_BOTTOM,
            vec2(16.0, -(theme::PLAYER_BAR_HEIGHT + 16.0)),
        )
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for job in &app.twerkz.jobs {
                card(ui, &palette, &art, job, &mut actions);
            }
        });
    for action in actions {
        app.actions.push(crate::model::Action::Twerkz(action));
    }
}

fn card(
    ui: &mut Ui,
    palette: &theme::Palette,
    art: &crate::images::ArtLoader,
    job: &Job,
    actions: &mut Vec<Action>,
) {
    Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(12))
        .shadow(egui::epaint::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: palette.shadow,
        })
        .show(ui, |ui| {
            ui.set_width(330.0);
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), egui::Sense::hover());
                crate::ui::widgets::paint_cover(ui, palette, job.cover.as_deref(), rect, 6.0, Icon::Music, Some(art));
                let badge = egui::Rect::from_center_size(rect.right_bottom() - vec2(4.0, 4.0), vec2(20.0, 20.0));
                if job.running {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(6), egui::Color32::from_black_alpha(110));
                    egui::Spinner::new()
                        .size(20.0)
                        .color(egui::Color32::WHITE)
                        .paint_at(ui, egui::Rect::from_center_size(rect.center(), vec2(20.0, 20.0)));
                } else {
                    let (icon, color) = if job.error {
                        (Icon::CircleAlert, palette.danger)
                    } else {
                        (Icon::CircleCheck, palette.accent)
                    };
                    ui.painter().circle_filled(badge.center(), 10.0, palette.overlay);
                    theme::paint_icon(ui, icon, badge, 18.0, color);
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 30.0);
                    truncated(ui, &job.heading, theme::medium(11.5), palette.dim);
                    let title = if job.running || job.title.is_empty() {
                        job.title.as_str()
                    } else if job.total > 1 {
                        job.summary.as_str()
                    } else {
                        job.title.as_str()
                    };
                    truncated(ui, title, theme::semibold(14.0), palette.text);
                    if job.running && !job.artist.is_empty() {
                        truncated(ui, &job.artist, theme::regular(12.5), palette.secondary);
                    }
                    let detail = if job.running {
                        progress_text(job)
                    } else if job.total > 1 && !job.title.is_empty() {
                        String::new()
                    } else {
                        job.summary.clone()
                    };
                    if !detail.is_empty() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(detail)
                                    .font(theme::medium(11.5))
                                    .color(if job.error && !job.running { palette.danger } else { palette.secondary }),
                            )
                            .wrap(),
                        );
                    }
                    if job.running && job.total > 1 {
                        ui.add_space(3.0);
                        let fraction = job.done as f32 / job.total as f32;
                        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
                        ui.painter().rect_filled(bar, CornerRadius::same(2), palette.surface_active);
                        let mut filled = bar;
                        filled.set_width(bar.width() * fraction);
                        ui.painter().rect_filled(filled, CornerRadius::same(2), palette.accent);
                    }
                    if !job.running
                        && let Some(folder) = &job.folder
                    {
                        ui.add_space(4.0);
                        if small_button(ui, palette, "Open folder") {
                            actions.push(Action::OpenFolder(folder.clone()));
                        }
                    }
                });
                let tip = if job.running { "Cancel" } else { "Close" };
                if theme::icon_button(ui, Icon::X, 14.0, palette.secondary, palette.text, tip).clicked() {
                    actions.push(if job.running {
                        Action::Cancel(job.id)
                    } else {
                        Action::Dismiss(job.id)
                    });
                }
            });
        });
}

/// "3 of 20 · 2 added · 1 failed" while a job runs.
fn progress_text(job: &Job) -> String {
    if job.total <= 1 {
        return String::new();
    }
    let mut parts = vec![format!("{} of {}", job.done, job.total)];
    let verb = match job.kind {
        Kind::Download => "saved",
        Kind::Import => "added",
    };
    if job.saved > 0 || matches!(job.kind, Kind::Import) {
        parts.push(format!("{} {verb}", job.saved));
    }
    if job.failed > 0 {
        parts.push(format!("{} failed", job.failed));
    }
    parts.join(" · ")
}

fn small_button(ui: &mut Ui, palette: &theme::Palette, label: &str) -> bool {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .font(theme::medium(12.0))
                .color(palette.text),
        )
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(6)),
    )
    .clicked()
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
