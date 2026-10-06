//! Menu entries, the import dialog and the job cards.

use std::path::PathBuf;
use std::time::Instant;

use egui::{Align2, CornerRadius, Frame, Margin, Stroke, Ui, vec2};

use super::import::{Collection, Found, Preview};
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
    OpenImport {
        playlist_id: String,
        playlist_name: String,
    },
}

/// The header button that adds YouTube, SoundCloud or local songs to a
/// playlist the account owns.
pub fn import_button(ui: &mut Ui, app: &mut App, playlist: &crate::api::models::Playlist) {
    let palette = app.palette;
    if theme::icon_button(
        ui,
        Icon::MusicPlus,
        26.0,
        palette.secondary,
        palette.text,
        "Add songs from YouTube, SoundCloud or local files",
    )
    .clicked()
    {
        app.actions.push(crate::model::Action::Twerkz(Action::OpenImport {
            playlist_id: playlist.id.clone(),
            playlist_name: playlist.name.clone(),
        }));
    }
}

#[derive(Default)]
pub struct State {
    jobs: Vec<Job>,
    next_id: u64,
    last_folder: Option<PathBuf>,
    import: Option<Import>,
    /// The emoji font the next start will use, read once Settings shows it.
    emoji_next: Option<Option<String>>,
    romaji: Option<Romaji>,
    romaji_on: bool,
    scanned: bool,
    reloaded: Option<Instant>,
}

struct Import {
    id: u64,
    playlist_id: String,
    playlist_name: String,
    url: String,
    stage: Stage,
    tab: Tab,
    filter: String,
    picked: std::collections::BTreeSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    YouTube,
    SoundCloud,
    Local,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::YouTube, Tab::SoundCloud, Tab::Local];

    fn label(self) -> &'static str {
        match self {
            Tab::YouTube => "YouTube",
            Tab::SoundCloud => "SoundCloud",
            Tab::Local => "Local",
        }
    }

    /// A link this tab takes, made whole.
    fn link(self, text: &str) -> Option<String> {
        let url = super::import::normalize_link(text)?;
        let soundcloud = url.to_lowercase().contains("soundcloud.com");
        match self {
            Tab::YouTube if !soundcloud => Some(url),
            Tab::SoundCloud if soundcloud => Some(url),
            _ => None,
        }
    }
}

enum Stage {
    Link { error: Option<String> },
    Loading(String),
    /// One song: its names can be changed before it is added.
    Song {
        preview: Preview,
        title: String,
        artist: String,
    },
    /// A playlist, album or set: every song's names can be changed.
    List { collection: Collection, rows: Vec<Row> },
    /// The songs being added, or added.
    Running {
        rows: Vec<Row>,
        done: Option<Result<String, String>>,
    },
    /// Local songs added.
    Done(Result<String, String>),
}

struct Row {
    preview: Preview,
    title: String,
    artist: String,
    state: RowState,
    editing: bool,
    picked: bool,
}

impl Row {
    fn new(preview: Preview) -> Self {
        Row {
            title: preview.title.clone(),
            artist: preview.artist.clone(),
            preview,
            state: RowState::Waiting,
            editing: false,
            picked: true,
        }
    }

    /// The song as it will be saved.
    fn song(&self) -> Preview {
        Preview {
            title: self.title.trim().to_string(),
            artist: self.artist.trim().to_string(),
            ..self.preview.clone()
        }
    }
}

#[derive(Clone, PartialEq)]
enum RowState {
    Waiting,
    Working,
    Added,
    Failed(String),
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
        Action::OpenImport {
            playlist_id,
            playlist_name,
        } => {
            app.twerkz.next_id += 1;
            app.twerkz.import = Some(Import {
                id: app.twerkz.next_id,
                playlist_id,
                playlist_name,
                url: String::new(),
                stage: Stage::Link { error: None },
                tab: Tab::YouTube,
                filter: String::new(),
                picked: Default::default(),
            });
        }
    }
}

/// Starts adding `rows` to the playlist, with a card that follows it.
fn start_import(app: &mut App, rows: &[Row]) -> Option<Request> {
    let import = app.twerkz.import.as_mut()?;
    // A fresh id, so a later import from the same dialog is its own job.
    app.twerkz.next_id += 1;
    import.id = app.twerkz.next_id;
    let mut job = Job::new(
        import.id,
        Kind::Import,
        format!("Adding to “{}”", import.playlist_name),
        "Starting…",
    );
    job.total = rows.len();
    job.cover = rows.first().and_then(|row| row.preview.thumbnail.clone());
    app.twerkz.jobs.push(job);
    Some(Request::Import {
        id: import.id,
        playlist_id: import.playlist_id.clone(),
        songs: rows.iter().map(Row::song).collect(),
    })
}

fn on_import_event(app: &mut App, event: Event) {
    match &event {
        Event::Imported {
            id,
            playlist_id,
            index,
            total,
            step,
        } => {
            on_imported(app, *id, playlist_id, *index, *total, step);
        }
        Event::ImportStatus { id, text } => {
            if let Some(job) = app.twerkz.jobs.iter_mut().find(|job| job.id == *id)
                && job.running
                && job.done == 0
            {
                job.title = text.clone();
            }
        }
        _ => {}
    }
    let Some(import) = app.twerkz.import.as_mut() else {
        return;
    };
    match event {
        Event::ImportPreview { id, result } if id == import.id => {
            import.stage = match result {
                Ok(Found::Song(preview)) => Stage::Song {
                    title: preview.title.clone(),
                    artist: preview.artist.clone(),
                    preview,
                },
                Ok(Found::Collection(collection)) => Stage::List {
                    rows: collection.entries.iter().cloned().map(Row::new).collect(),
                    collection,
                },
                Err(error) => Stage::Link { error: Some(error) },
            };
        }
        Event::ImportStatus { id, text } if id == import.id => {
            if let Stage::Loading(status) = &mut import.stage {
                *status = text;
            }
        }
        Event::Imported { id, index, step, .. } if id == import.id => {
            if let Stage::Running { rows, done } = &mut import.stage {
                match step {
                    Step::Working { .. } => {
                        if let Some(row) = rows.get_mut(index) {
                            row.state = RowState::Working;
                        }
                    }
                    Step::Added => {
                        if let Some(row) = rows.get_mut(index) {
                            row.state = RowState::Added;
                        }
                    }
                    Step::Failed(error) => {
                        if let Some(row) = rows.get_mut(index) {
                            row.state = RowState::Failed(error);
                        }
                    }
                    Step::Finished(result) => {
                        for row in rows.iter_mut() {
                            if row.state == RowState::Working {
                                row.state = RowState::Waiting;
                            }
                        }
                        let added = rows.iter().filter(|row| row.state == RowState::Added).count();
                        *done = Some(match result {
                            Ok(true) => Err(format!("Cancelled · {}", songs(added, "added"))),
                            Ok(false) => import_summary(rows),
                            Err(error) => Err(error),
                        });
                    }
                }
            }
        }
        Event::AddedLocal {
            id,
            playlist_id,
            result,
        } if id == import.id => {
            if result.is_ok() {
                import.picked.clear();
                app.actions.push(crate::model::Action::Reload(crate::model::Page::Playlist(
                    playlist_id,
                )));
            }
            import.stage = Stage::Done(result);
        }
        _ => {}
    }
}

fn songs(count: usize, what: &str) -> String {
    match count {
        1 => format!("1 song {what}"),
        n => format!("{n} songs {what}"),
    }
}

fn import_summary(rows: &[Row]) -> Result<String, String> {
    let added = rows.iter().filter(|row| row.state == RowState::Added).count();
    let failed: Vec<&Row> = rows
        .iter()
        .filter(|row| matches!(row.state, RowState::Failed(_)))
        .collect();
    match (added, failed.len()) {
        (_, 0) => Ok(songs(added, "added")),
        (0, 1) => Err(match &failed[0].state {
            RowState::Failed(error) => error.clone(),
            _ => String::new(),
        }),
        (added, failed) => Err(format!("{} · {failed} failed", songs(added, "added"))),
    }
}

fn on_imported(app: &mut App, id: u64, playlist_id: &str, index: usize, total: usize, step: &Step) {
    let reload = crate::model::Action::Reload(crate::model::Page::Playlist(playlist_id.to_string()));
    let Some(job) = app.twerkz.jobs.iter_mut().find(|job| job.id == id) else {
        return;
    };
    job.total = total;
    match step {
        Step::Working { title, artist, cover } => {
            job.title = title.clone();
            job.artist = artist.clone();
            if cover.is_some() {
                job.cover = cover.clone();
            }
            job.done = index;
        }
        Step::Added => {
            job.saved += 1;
            job.done = index + 1;
            // The playlist shows each song as it lands, a few seconds apart.
            if app
                .twerkz
                .reloaded
                .is_none_or(|last| last.elapsed().as_secs() >= 3)
            {
                app.twerkz.reloaded = Some(Instant::now());
                app.actions.push(reload);
            }
        }
        Step::Failed(_) => {
            job.failed += 1;
            job.done = index + 1;
        }
        Step::Finished(result) => {
            let added = job.saved;
            match result {
                Ok(cancelled) => {
                    let mut parts = vec![if *cancelled {
                        "Cancelled".to_string()
                    } else {
                        "Done".to_string()
                    }];
                    parts.push(songs(added, "added"));
                    if job.failed > 0 {
                        parts.push(format!("{} failed", job.failed));
                    }
                    let error = job.failed > 0;
                    job.end(error, parts.join(" · "));
                }
                Err(error) => job.end(true, error.clone()),
            }
            if added > 0 {
                app.actions.push(reload);
                // The engine reads its local files when it starts, so the
                // new songs play only after a restart, which also rescans.
                app.actions.push(crate::model::Action::RestartEngine);
            }
        }
    }
}

/// The import dialog: YouTube and SoundCloud take a link to a song or a
/// whole list, whose names can be changed first; Local lists the songs in
/// the local folders.
fn import_dialog(app: &mut App, ctx: &egui::Context) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(playlist_id) = app.twerkz.import.as_ref().map(|import| import.playlist_id.clone()) else {
        return;
    };
    let index = app.local_index.clone();
    let in_playlist: std::collections::HashSet<String> = app
        .playlist_pages
        .get(&playlist_id)
        .map(|page| {
            page.items
                .items
                .iter()
                .filter_map(|row| row.playable().map(|item| item.uri().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let art = app.backend.art().clone();
    let Some(import) = app.twerkz.import.as_mut() else {
        return;
    };
    let mut close = false;
    let mut send = None;
    let mut start: Option<Vec<Row>> = None;
    let mut cancel = None;
    let frame = Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS + 4))
        .inner_margin(Margin::same(24))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 40,
            spread: 0,
            color: palette.shadow,
        });
    let modal = egui::Modal::new(egui::Id::new("twerkz-import"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(if palette.dark { 150 } else { 80 }))
        .show(ctx, |ui| {
            ui.set_width(500.0);
            theme::text(ui, "Add songs", theme::bold(20.0), palette.text);
            ui.add_space(4.0);
            theme::text(
                ui,
                format!("to {}", import.playlist_name),
                theme::medium(13.0),
                palette.secondary,
            );
            ui.add_space(12.0);
            let locked = matches!(import.stage, Stage::Loading(_) | Stage::Running { done: None, .. });
            ui.horizontal(|ui| {
                for tab in Tab::ALL {
                    if theme::soft_button(ui, &palette, None, tab.label(), import.tab == tab).clicked()
                        && !locked
                        && import.tab != tab
                    {
                        import.tab = tab;
                        import.url.clear();
                        import.stage = Stage::Link { error: None };
                    }
                }
            });
            ui.add_space(14.0);
            match &mut import.stage {
                Stage::Link { .. } if import.tab == Tab::Local => {
                    local_list(
                        ui,
                        &palette,
                        locale,
                        index.as_deref(),
                        &in_playlist,
                        &mut import.filter,
                        &mut import.picked,
                    );
                    ui.add_space(14.0);
                    let count = import.picked.len();
                    let mut add = false;
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = match count {
                            0 => "Add songs".to_string(),
                            1 => "Add 1 song".to_string(),
                            n => format!("Add {n} songs"),
                        };
                        add = ui
                            .add_enabled_ui(count > 0, |ui| theme::pill_button(ui, &palette, &label, true))
                            .inner
                            .clicked();
                        close |= theme::pill_button(ui, &palette, "Cancel", false).clicked();
                    });
                    if add {
                        send = Some(Request::AddLocal {
                            id: import.id,
                            playlist_id: import.playlist_id.clone(),
                            uris: import.picked.iter().cloned().collect(),
                        });
                        import.stage = Stage::Loading("Adding to the playlist…".to_string());
                    }
                }
                Stage::Link { error } => {
                    let hint = match import.tab {
                        Tab::SoundCloud => "Paste a SoundCloud song, set or profile link",
                        _ => "Paste a YouTube song or playlist link",
                    };
                    let field = field(ui, &palette, locale, "twerkz-import-url", &mut import.url, hint, true);
                    let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if let Some(error) = error {
                        ui.add_space(8.0);
                        wrapped(ui, error.as_str(), theme::medium(12.5), palette.danger);
                    }
                    ui.add_space(16.0);
                    let mut go = entered;
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        go |= theme::pill_button(ui, &palette, "Find", true).clicked();
                        close |= theme::pill_button(ui, &palette, "Cancel", false).clicked();
                    });
                    if go {
                        match import.tab.link(&import.url) {
                            Some(url) => {
                                send = Some(Request::ImportPreview { id: import.id, url });
                                import.stage = Stage::Loading("Reading the link…".to_string());
                            }
                            None => {
                                *error = Some(format!("That isn't a {} link.", import.tab.label()));
                            }
                        }
                    }
                }
                Stage::Song {
                    preview,
                    title,
                    artist,
                } => {
                    ui.horizontal(|ui| {
                        crate::ui::widgets::cover(ui, &palette, preview.thumbnail.as_deref(), 64.0, 6.0, Icon::Music);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.add_space(6.0);
                            truncated(ui, &preview.title, theme::semibold(14.5), palette.text);
                            theme::text(
                                ui,
                                format!(
                                    "{} · {}:{:02}",
                                    preview.source,
                                    preview.seconds / 60,
                                    preview.seconds % 60
                                ),
                                theme::medium(12.0),
                                palette.secondary,
                            );
                        });
                    });
                    ui.add_space(14.0);
                    theme::text(ui, "Title", theme::semibold(12.0), palette.secondary);
                    field(ui, &palette, locale, "twerkz-import-title", title, "Title", true);
                    ui.add_space(10.0);
                    theme::text(ui, "Artist", theme::semibold(12.0), palette.secondary);
                    field(ui, &palette, locale, "twerkz-import-artist", artist, "Artist", false);
                    ui.add_space(16.0);
                    let ready = !title.trim().is_empty() && !artist.trim().is_empty();
                    let (mut add, mut back) = (false, false);
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        add = ui
                            .add_enabled_ui(ready, |ui| theme::pill_button(ui, &palette, "Download and add", true))
                            .inner
                            .clicked();
                        back = theme::pill_button(ui, &palette, "Back", false).clicked();
                    });
                    if add {
                        let mut row = Row::new(preview.clone());
                        row.title = title.clone();
                        row.artist = artist.clone();
                        start = Some(vec![row]);
                    } else if back {
                        import.stage = Stage::Link { error: None };
                    }
                }
                Stage::List { collection, rows } => {
                    ui.horizontal(|ui| {
                        crate::ui::widgets::cover(ui, &palette, collection.cover.as_deref(), 64.0, 6.0, Icon::ListMusic);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.add_space(6.0);
                            truncated(ui, &collection.title, theme::semibold(14.5), palette.text);
                            let mut detail = format!("{} · {}", collection.source, songs(rows.len(), ""));
                            if collection.mix {
                                detail = format!("{} · first songs of a mix", detail.trim_end());
                            }
                            theme::text(ui, detail.trim_end(), theme::medium(12.0), palette.secondary);
                        });
                    });
                    ui.add_space(10.0);
                    song_rows(ui, &palette, locale, &art, rows, true);
                    ui.add_space(14.0);
                    let picked = rows.iter().filter(|row| row.picked).count();
                    let ready = picked > 0
                        && rows
                            .iter()
                            .filter(|row| row.picked)
                            .all(|row| !row.title.trim().is_empty() && !row.artist.trim().is_empty());
                    let (mut add, mut back) = (false, false);
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = format!("Download and add {}", songs(picked, "").trim_end());
                        add = ui
                            .add_enabled_ui(ready, |ui| theme::pill_button(ui, &palette, &label, true))
                            .inner
                            .clicked();
                        back = theme::pill_button(ui, &palette, "Back", false).clicked();
                    });
                    if add {
                        start = Some(
                            std::mem::take(rows)
                                .into_iter()
                                .filter(|row| row.picked)
                                .map(|row| Row { editing: false, ..row })
                                .collect(),
                        );
                    } else if back {
                        import.stage = Stage::Link { error: None };
                    }
                }
                Stage::Running { rows, done } => {
                    song_rows(ui, &palette, locale, &art, rows, false);
                    ui.add_space(12.0);
                    match done {
                        None => {
                            let added = rows.iter().filter(|row| row.state == RowState::Added).count();
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0).color(palette.accent));
                                theme::text(
                                    ui,
                                    format!("{} of {} added", added, rows.len()),
                                    theme::medium(13.0),
                                    palette.text,
                                );
                            });
                            ui.add_space(12.0);
                            ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                close |= theme::pill_button(ui, &palette, "Hide", true).clicked();
                                if theme::pill_button(ui, &palette, "Cancel import", false).clicked() {
                                    cancel = Some(import.id);
                                }
                            });
                        }
                        Some(result) => {
                            outcome(ui, &palette, result);
                            ui.add_space(16.0);
                            ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                close |= theme::pill_button(ui, &palette, "Done", true).clicked();
                                if theme::pill_button(ui, &palette, "Add more", false).clicked() {
                                    import.url.clear();
                                    import.stage = Stage::Link { error: None };
                                }
                            });
                        }
                    }
                }
                Stage::Loading(status) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().color(palette.accent));
                        theme::text(ui, status.as_str(), theme::medium(13.5), palette.text);
                    });
                    ui.add_space(8.0);
                }
                Stage::Done(result) => {
                    outcome(ui, &palette, result);
                    ui.add_space(16.0);
                    let failed = result.is_err();
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close |= theme::pill_button(ui, &palette, "Done", true).clicked();
                        if theme::pill_button(ui, &palette, if failed { "Try again" } else { "Add more" }, false)
                            .clicked()
                        {
                            import.stage = Stage::Link { error: None };
                        }
                    });
                }
            }
            if locked {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        });
    let loading = app
        .twerkz
        .import
        .as_ref()
        .is_some_and(|import| matches!(import.stage, Stage::Loading(_)));
    if modal.should_close() && !loading {
        close = true;
    }
    if let Some(rows) = start {
        if let Some(request) = start_import(app, &rows)
            && let Some(import) = app.twerkz.import.as_mut()
        {
            import.stage = Stage::Running { rows, done: None };
            send = Some(request);
        }
    }
    if let Some(id) = cancel {
        app.actions.push(crate::model::Action::Twerkz(Action::Cancel(id)));
    }
    if let Some(request) = send {
        app.backend.send(Command::Twerkz(request));
    }
    if close {
        app.twerkz.import = None;
    }
}

fn outcome(ui: &mut Ui, palette: &theme::Palette, result: &Result<String, String>) {
    let (icon, color, text) = match result {
        Ok(text) => (Icon::CircleCheck, palette.accent, text.as_str()),
        Err(error) => (Icon::CircleAlert, palette.danger, error.as_str()),
    };
    ui.horizontal(|ui| {
        theme::icon(ui, icon, 18.0, color);
        wrapped(ui, text, theme::medium(13.5), palette.text);
    });
}

fn wrapped(ui: &mut Ui, text: &str, font: egui::FontId, color: egui::Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap());
}

fn truncated(ui: &mut Ui, text: &str, font: egui::FontId, color: egui::Color32) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).truncate())
}

/// The songs of a list: number or state, cover, title and artist. While
/// `editable`, each row has a tick to pick it and a pencil to change its
/// names.
fn song_rows(
    ui: &mut Ui,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    art: &crate::images::ArtLoader,
    rows: &mut [Row],
    editable: bool,
) {
    egui::ScrollArea::vertical()
        .id_salt("twerkz-import-rows")
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (index, row) in rows.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    Frame::new()
                        .fill(if row.editing { palette.surface } else { egui::Color32::TRANSPARENT })
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (marker, _) = ui.allocate_exact_size(vec2(22.0, 40.0), egui::Sense::hover());
                                match &row.state {
                                    RowState::Waiting => {
                                        ui.painter().text(
                                            marker.center(),
                                            Align2::CENTER_CENTER,
                                            (index + 1).to_string(),
                                            theme::medium(12.0),
                                            palette.dim,
                                        );
                                    }
                                    RowState::Working => {
                                        egui::Spinner::new()
                                            .size(14.0)
                                            .color(palette.accent)
                                            .paint_at(ui, egui::Rect::from_center_size(marker.center(), vec2(14.0, 14.0)));
                                    }
                                    RowState::Added => {
                                        theme::paint_icon(ui, Icon::Check, marker, 16.0, palette.accent);
                                    }
                                    RowState::Failed(_) => {
                                        theme::paint_icon(ui, Icon::CircleAlert, marker, 16.0, palette.danger);
                                    }
                                }
                                let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                                crate::ui::widgets::paint_cover(
                                    ui,
                                    palette,
                                    row.preview.thumbnail.as_deref(),
                                    rect,
                                    4.0,
                                    Icon::Music,
                                    Some(art),
                                );
                                if !row.picked {
                                    ui.painter().rect_filled(rect, CornerRadius::same(4), palette.overlay.gamma_multiply(0.6));
                                }
                                ui.add_space(4.0);
                                let controls = if editable { 64.0 } else { 0.0 };
                                ui.allocate_ui_with_layout(
                                    vec2(ui.available_width() - controls, 40.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        if row.editing {
                                            small_field(ui, palette, locale, &mut row.title, "Title");
                                            ui.add_space(3.0);
                                            small_field(ui, palette, locale, &mut row.artist, "Artist");
                                        } else {
                                            let faded = matches!(row.state, RowState::Added) || !row.picked;
                                            ui.add_space(3.0);
                                            truncated(
                                                ui,
                                                &row.title,
                                                theme::medium(13.5),
                                                if faded { palette.secondary } else { palette.text },
                                            );
                                            let detail = match &row.state {
                                                RowState::Failed(error) => error.clone(),
                                                _ => row.artist.clone(),
                                            };
                                            let color = match row.state {
                                                RowState::Failed(_) => palette.danger,
                                                _ if faded => palette.dim,
                                                _ => palette.secondary,
                                            };
                                            truncated(ui, &detail, theme::regular(12.0), color);
                                        }
                                    },
                                );
                                if editable {
                                    let tip = if row.editing { "Done editing" } else { "Edit title and artist" };
                                    let pencil = if row.editing { Icon::Check } else { Icon::Pencil };
                                    if theme::icon_button(ui, pencil, 16.0, palette.secondary, palette.text, tip).clicked() {
                                        row.editing = !row.editing;
                                    }
                                    if tick(ui, palette, row.picked).clicked() {
                                        row.picked = !row.picked;
                                    }
                                }
                            });
                        });
                });
            }
        });
}

/// A round tick: filled green when picked, an empty ring when not.
fn tick(ui: &mut Ui, palette: &theme::Palette, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
    let response = response.on_hover_text(if on { "Skip this song" } else { "Download this song" });
    let center = rect.center();
    if on {
        ui.painter().circle_filled(center, 10.0, palette.accent);
        theme::paint_icon(ui, Icon::Check, rect, 13.0, palette.on_accent);
    } else {
        let color = if response.hovered() { palette.text } else { palette.dim };
        ui.painter().circle_stroke(center, 9.5, Stroke::new(1.5, color));
    }
    response
}

fn small_field(ui: &mut Ui, palette: &theme::Palette, locale: crate::i18n::Locale, text: &mut String, hint: &str) {
    Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            let _ = crate::ui::widgets::text_edit(
                ui,
                locale,
                egui::TextEdit::singleline(text)
                    .hint_text(egui::RichText::new(hint).color(palette.dim))
                    .font(theme::regular(12.5))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            );
        });
}

/// The songs in the local folders, filterable, each picked with a click.
/// Songs already in the playlist are shown but cannot be picked again.
fn local_list(
    ui: &mut Ui,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    index: Option<&crate::localfiles::Index>,
    in_playlist: &std::collections::HashSet<String>,
    filter: &mut String,
    picked: &mut std::collections::BTreeSet<String>,
) {
    let files = index.map(|index| index.files.as_slice()).unwrap_or_default();
    if files.is_empty() {
        let text = if index.is_none() {
            "Looking for local songs…"
        } else {
            "No local songs yet. Import some from YouTube or SoundCloud, or add folders under Settings, Local files."
        };
        wrapped(ui, text, theme::medium(13.0), palette.secondary);
        return;
    }
    field(ui, palette, locale, "twerkz-local-filter", filter, "Search local songs", true);
    ui.add_space(8.0);
    let needle = filter.trim().to_lowercase();
    let mut shown: Vec<&crate::localfiles::LocalFile> = files
        .iter()
        .filter(|file| {
            needle.is_empty()
                || [&file.title, &file.artist, &file.album]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
        })
        .collect();
    shown.sort_by_key(|file| file.title.to_lowercase());
    egui::ScrollArea::vertical()
        .id_salt("twerkz-local-songs")
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for file in shown {
                let added = in_playlist.contains(&file.uri);
                let selected = picked.contains(&file.uri);
                let subtitle = if file.artist.is_empty() {
                    file.album.clone()
                } else {
                    file.artist.clone()
                };
                ui.push_id(&file.uri, |ui| {
                    ui.add_enabled_ui(!added, |ui| {
                        let response = Frame::new()
                            .fill(if selected { palette.surface_active } else { egui::Color32::TRANSPARENT })
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(8, 5))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                                    crate::ui::widgets::paint_cover(
                                        ui,
                                        palette,
                                        Some(file.uri.as_str()),
                                        rect,
                                        4.0,
                                        Icon::Music,
                                        None,
                                    );
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.add_space(3.0);
                                        truncated(ui, &file.title, theme::medium(13.5), palette.text);
                                        let detail = if added {
                                            format!("{subtitle} · already in this playlist")
                                        } else {
                                            subtitle.clone()
                                        };
                                        truncated(ui, &detail, theme::regular(12.0), palette.secondary);
                                    });
                                    if selected || added {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            theme::icon(ui, Icon::CircleCheck, 18.0, palette.accent);
                                        });
                                    }
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if response.clicked() {
                            if selected {
                                picked.remove(&file.uri);
                            } else {
                                picked.insert(file.uri.clone());
                            }
                        }
                    });
                });
            }
        });
}

fn field(
    ui: &mut Ui,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
    id: &str,
    text: &mut String,
    hint: &str,
    focus: bool,
) -> egui::Response {
    let response = Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            crate::ui::widgets::text_edit(
                ui,
                locale,
                egui::TextEdit::singleline(text)
                    .id(egui::Id::new(id))
                    .hint_text(egui::RichText::new(hint).color(palette.dim))
                    .font(theme::regular(14.0))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            )
        })
        .inner;
    if focus && ui.memory(|memory| memory.focused().is_none()) {
        response.request_focus();
    }
    response
}

pub fn on_event(app: &mut App, event: Event) {
    let state = &mut app.twerkz;
    let id = match &event {
        Event::Dismissed { id }
        | Event::Status { id, .. }
        | Event::Progress { id, .. }
        | Event::Finished { id, .. }
        | Event::Failed { id, .. } => *id,
        Event::ImportPreview { .. }
        | Event::ImportStatus { .. }
        | Event::Imported { .. }
        | Event::AddedLocal { .. } => {
            on_import_event(app, event);
            return;
        }
        Event::EmojiFont { result } => {
            on_emoji_event(app, result.clone());
            return;
        }
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
        Event::ImportPreview { .. }
        | Event::ImportStatus { .. }
        | Event::Imported { .. }
        | Event::AddedLocal { .. }
        | Event::EmojiFont { .. }
        | Event::Romanized { .. } => {}
    }
}

/// Keeps the local covers in step with the index, and reads the local files
/// once at startup so playlists show and play them straight away.
fn local_files(app: &mut App) {
    super::covers::set_index(app.local_index.clone());
    if app.twerkz.scanned {
        return;
    }
    app.twerkz.scanned = true;
    if app.local_index.is_none() && !app.local_files_scanning && !app.settings.local_folders.is_empty() {
        app.local_files_scanning = true;
        app.backend.send(Command::LocalFilesScan);
    }
}

/// The job cards, bottom left above the player bar: the song being worked
/// on with its cover, how far along the job is, and a way to stop it.
pub fn panel(app: &mut App, ctx: &egui::Context) {
    local_files(app);
    import_dialog(app, ctx);
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

/// The emoji font row in Settings. Returns whether it is shown under the
/// search in the settings filter.
pub fn emoji_settings(app: &mut App, ui: &mut Ui, needle: &str) -> bool {
    const TITLE: &str = "Emoji font";
    const DESCRIPTION: &str =
        "Draw emoji with a font file of your own, such as Apple Color Emoji. Applies after a restart.";
    let needle = needle.trim().to_lowercase();
    if !needle.is_empty()
        && !["emoji", TITLE, DESCRIPTION]
            .iter()
            .any(|text| text.to_lowercase().contains(&needle))
    {
        return false;
    }
    let palette = app.palette;
    let next = app
        .twerkz
        .emoji_next
        .get_or_insert_with(super::emoji::next_name)
        .clone();
    crate::ui::settings::section(ui, &palette, "Emoji", |ui| {
        crate::ui::widgets::setting_row(ui, &palette, TITLE, DESCRIPTION, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                theme::text(
                    ui,
                    next.as_deref().unwrap_or("System emoji"),
                    theme::medium(13.0),
                    palette.secondary,
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if theme::soft_button(ui, &palette, Some(Icon::FolderOpen), "Open folder", false)
                        .clicked()
                    {
                        app.actions
                            .push(crate::model::Action::Twerkz(Action::OpenFolder(super::emoji::folder())));
                    }
                    if next.is_some()
                        && theme::soft_button(ui, &palette, None, "Use system emoji", false).clicked()
                    {
                        match super::emoji::choose_system() {
                            Ok(()) => {
                                app.twerkz.emoji_next = Some(None);
                                app.toast("System emoji from the next start");
                            }
                            Err(error) => app.toast_error(format!("Couldn't change the emoji font: {error}")),
                        }
                    }
                    if theme::soft_button(ui, &palette, None, "Choose font…", false).clicked() {
                        let file = rfd::AsyncFileDialog::new()
                            .set_title("Choose an emoji font")
                            .add_filter("Font", &["ttf", "ttc"])
                            .pick_file();
                        app.backend
                            .send(Command::Twerkz(Request::ChooseEmojiFont { file: Box::pin(file) }));
                    }
                });
            });
        });
    });
    true
}

fn on_emoji_event(app: &mut App, result: Result<Option<String>, String>) {
    match result {
        Ok(Some(name)) => {
            app.twerkz.emoji_next = Some(Some(name.clone()));
            app.toast(format!("{name} from the next start"));
        }
        Ok(None) => {}
        Err(error) => app.toast_error(format!("Couldn't use that font: {error}")),
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
