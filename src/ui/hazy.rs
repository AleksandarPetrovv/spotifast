//! The Hazy theme's options, in a popup opened by the top bar's pencil.

use std::sync::{Arc, Mutex};

use egui::{Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Sense, Stroke, Ui, UiBuilder, vec2};

use crate::app::App;
use crate::i18n::gettext;
use crate::model::Action;
use crate::settings::HazySettings;
use crate::theme::{self, Icon};

const FADE: f64 = 0.18;
const STAGGER: f64 = 0.045;
const ROW_FADE: f64 = 0.22;
const ROWS: usize = 9;
const WIDTH: f32 = 460.0;
const LABEL_WIDTH: f32 = 100.0;

#[derive(Clone, Copy)]
struct State {
    opened: f64,
    closed: Option<f64>,
}

fn state_id() -> Id {
    Id::new("hazy-popup")
}

fn now(ctx: &egui::Context) -> f64 {
    ctx.input(|input| input.time)
}

pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<State>(state_id()))
        .is_some_and(|state| state.closed.is_none())
}

pub fn open(ctx: &egui::Context) {
    if !is_open(ctx) {
        let opened = now(ctx);
        ctx.data_mut(|data| {
            data.insert_temp(
                state_id(),
                State {
                    opened,
                    closed: None,
                },
            )
        });
    }
    ctx.request_repaint();
}

fn close(ctx: &egui::Context) {
    let closed = now(ctx);
    ctx.data_mut(|data| {
        if let Some(mut state) = data.get_temp::<State>(state_id())
            && state.closed.is_none()
        {
            state.closed = Some(closed);
            data.insert_temp(state_id(), state);
        }
    });
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(state) = ctx.data(|data| data.get_temp::<State>(state_id())) else {
        return;
    };
    let time = now(ctx);
    let master = match state.closed {
        None => ((time - state.opened) / FADE).clamp(0.0, 1.0),
        Some(closed) => 1.0 - ((time - closed) / FADE).clamp(0.0, 1.0),
    } as f32;
    if !app.settings.hazy_active() || (state.closed.is_some() && master <= 0.0) {
        ctx.data_mut(|data| data.remove::<State>(state_id()));
        return;
    }
    let age = time - state.opened;
    if state.closed.is_some() || age < FADE + STAGGER * ROWS as f64 + ROW_FADE {
        ctx.request_repaint();
    }

    let dark = app.palette.dark;
    let any_popup_open = egui::Popup::is_any_open(ctx);
    let area = egui::Modal::default_area(Id::new("hazy-popup-area"));
    let is_top = ctx.memory_mut(|memory| {
        memory.set_modal_layer(area.layer());
        memory.top_modal_layer() == Some(area.layer())
    });
    let frame = Frame::new()
        .fill(Color32::from_rgba_unmultiplied(16, 18, 24, 214))
        .stroke(Stroke::new(1.0, Color32::from_white_alpha(32)))
        .corner_radius(CornerRadius::same(22))
        .inner_margin(Margin::same(26))
        .shadow(egui::epaint::Shadow {
            offset: [0, 14],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(120),
        });
    let shown = area.show(ctx, |ui| {
        ui.multiply_opacity(master);
        let bounds = ui.ctx().content_rect();
        let mut backdrop = ui.new_child(
            UiBuilder::new()
                .sense(Sense::CLICK | Sense::DRAG)
                .max_rect(bounds),
        );
        backdrop.set_min_size(bounds.size());
        ui.painter().rect_filled(
            bounds,
            0.0,
            Color32::from_black_alpha(if dark { 150 } else { 100 }),
        );
        let backdrop = backdrop.response();
        let close_clicked = ui
            .scope_builder(UiBuilder::new().sense(Sense::CLICK | Sense::DRAG), |ui| {
                frame.show(ui, |ui| panel(app, ui, age)).inner
            })
            .inner;
        (backdrop, close_clicked)
    });
    let (backdrop, close_clicked) = shown.inner;
    let escape = is_top
        && !any_popup_open
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if state.closed.is_none() && (backdrop.clicked() || close_clicked || escape) {
        close(ctx);
    }
}

fn fade_row(ui: &mut Ui, age: f64, index: usize, add: impl FnOnce(&mut Ui)) {
    let fraction = ((age - FADE * 0.5 - STAGGER * index as f64) / ROW_FADE).clamp(0.0, 1.0) as f32;
    ui.scope(|ui| {
        ui.multiply_opacity(1.0 - (1.0 - fraction).powi(3));
        add(ui);
    });
}

fn label(ui: &mut Ui, text: String) {
    theme::text(ui, text, theme::medium(14.0), Color32::WHITE);
}

fn panel(app: &mut App, ui: &mut Ui, age: f64) -> bool {
    let palette = app.palette;
    let locale = app.locale;
    let mut hazy = app.settings.hazy.clone();
    let mut close = false;
    ui.set_width(WIDTH);
    ui.spacing_mut().item_spacing.y = 16.0;

    fade_row(ui, age, 0, |ui| {
        ui.horizontal(|ui| {
            theme::text(
                ui,
                gettext(locale, "Edit Theme"),
                theme::bold(20.0),
                Color32::WHITE,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = theme::icon_button(
                    ui,
                    Icon::X,
                    18.0,
                    Color32::from_white_alpha(170),
                    Color32::WHITE,
                    &gettext(locale, "Close"),
                )
                .clicked();
            });
        });
    });

    fade_row(ui, age, 1, |ui| {
        ui.horizontal(|ui| {
            label(ui, gettext(locale, "Custom Background").into_owned());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                super::widgets::switch(
                    ui,
                    &palette,
                    &gettext(locale, "Custom Background"),
                    &mut hazy.custom_background,
                );
            });
        });
    });

    if hazy.custom_background {
        fade_row(ui, age, 2, |ui| {
            ui.horizontal(|ui| {
                if theme::soft_button(
                    ui,
                    &palette,
                    Some(Icon::FolderOpen),
                    &gettext(locale, "Choose Image…"),
                    false,
                )
                .clicked()
                {
                    app.actions.push(Action::ChooseHazyBackground);
                }
                if let Some(path) = hazy.background.as_deref() {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(texture) = preview(ui, path) {
                            ui.add(
                                egui::Image::new(&texture)
                                    .fit_to_exact_size(vec2(72.0, 44.0))
                                    .corner_radius(8),
                            );
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            theme::text(
                                ui,
                                name,
                                theme::regular(13.0),
                                Color32::from_white_alpha(170),
                            );
                        });
                    });
                }
            });
        });
    }

    let sliders: [(&str, &mut u8, u8, &str); 4] = [
        ("Blur", &mut hazy.blur, 50, " px"),
        ("Contrast", &mut hazy.contrast, 200, " %"),
        ("Saturation", &mut hazy.saturation, 200, " %"),
        ("Brightness", &mut hazy.brightness, 200, " %"),
    ];
    for (index, (name, value, max, unit)) in sliders.into_iter().enumerate() {
        fade_row(ui, age, 3 + index, |ui| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(LABEL_WIDTH, 28.0),
                    Layout::left_to_right(Align::Center),
                    |ui| label(ui, gettext(locale, name).into_owned()),
                );
                ui.spacing_mut().slider_width = (ui.available_width() - 100.0).max(80.0);
                super::settings::setting_slider(
                    ui,
                    &palette,
                    value,
                    0..=max,
                    |slider| slider,
                    |drag| drag.suffix(unit),
                );
            });
        });
    }

    fade_row(ui, age, 7, |ui| {
        ui.horizontal(|ui| {
            label(ui, gettext(locale, "Custom Color").into_owned());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                super::widgets::switch(
                    ui,
                    &palette,
                    &gettext(locale, "Custom Color"),
                    &mut hazy.custom_color,
                );
                if hazy.custom_color {
                    egui::color_picker::color_edit_button_srgb(ui, &mut hazy.color);
                }
            });
        });
    });

    fade_row(ui, age, 8, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::soft_button(ui, &palette, None, &gettext(locale, "Reset"), false).clicked() {
                hazy = HazySettings::default();
            }
        });
    });

    if hazy != app.settings.hazy {
        app.actions.push(Action::SetHazy(hazy));
    }
    close
}

#[derive(Clone)]
struct Preview {
    path: String,
    pending: Arc<Mutex<Option<egui::ColorImage>>>,
    texture: Option<egui::TextureHandle>,
}

/// Decodes off the UI thread: a photo can take longer than a frame.
fn preview(ui: &Ui, path: &str) -> Option<egui::TextureHandle> {
    let id = Id::new("hazy-preview");
    let ctx = ui.ctx();
    let mut state = ctx
        .data(|data| data.get_temp::<Preview>(id))
        .filter(|state| state.path == path)
        .unwrap_or_else(|| {
            let pending = Arc::new(Mutex::new(None));
            let slot = Arc::clone(&pending);
            let file = path.to_owned();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                if let Ok(image) = image::open(file) {
                    let thumb = image.thumbnail(160, 160).to_rgba8();
                    let size = [thumb.width() as usize, thumb.height() as usize];
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, thumb.as_raw());
                    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(image);
                    repaint.request_repaint();
                }
            });
            Preview {
                path: path.to_owned(),
                pending,
                texture: None,
            }
        });
    if state.texture.is_none() {
        let image = state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(image) = image {
            state.texture = Some(ctx.load_texture("hazy-preview", image, egui::TextureOptions::LINEAR));
        }
    }
    let texture = state.texture.clone();
    ctx.data_mut(|data| data.insert_temp(id, state));
    texture
}
