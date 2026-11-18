//! The Hazy theme's backdrop: the playing cover (or a chosen picture), made
//! small and soft on the CPU, stretched behind every panel. Working from a
//! 128 pixel copy keeps the compression blocks of the source out of the blur.

use std::sync::{Arc, Mutex};

use egui::load::BytesPoll;
use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions, pos2};

use crate::app::App;
use crate::images::ArtLoader;

const BASE: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
const OPACITY: f32 = 0.55;
/// The long side of the working copy.
const SIDE: u32 = 128;
/// A CSS blur is in screen pixels; the working copy is about a tenth of a
/// window's width.
const BLUR_SCALE: f32 = 0.2;
const FADE_SECONDS: f64 = 0.4;

#[derive(Clone, PartialEq)]
enum Source {
    Art(String),
    File(String),
}

/// What a backdrop is made from: the picture and the filters.
#[derive(Clone, PartialEq)]
struct Key {
    source: Option<Source>,
    blur: u8,
    contrast: u8,
    saturation: u8,
    brightness: u8,
}

struct Layer {
    key: Key,
    texture: Option<TextureHandle>,
}

/// Every finished render, so a slower one for an older key cannot write
/// over the one still wanted.
type Finished = Arc<Mutex<Vec<(Key, Option<ColorImage>)>>>;

#[derive(Default)]
pub struct Backdrop {
    wanted: Option<Key>,
    /// The cover's bytes have not arrived yet.
    waiting: bool,
    finished: Finished,
    below: Option<Layer>,
    above: Option<Layer>,
    since: f64,
}

/// Paints the backdrop behind everything the window draws next, and lets
/// go of it while another theme is shown.
pub fn paint(app: &mut App, ui: &egui::Ui) {
    if !app.settings.hazy_active() {
        app.hazy = Backdrop::default();
        return;
    }
    let ctx = ui.ctx();
    let hazy = &app.settings.hazy;
    let chosen = hazy
        .custom_background
        .then(|| hazy.background.clone())
        .flatten()
        .filter(|path| !path.is_empty());
    let source = match chosen {
        Some(path) => Some(Source::File(path)),
        None => app
            .now_playing()
            .and_then(|now| now.art_url.or(now.art_small))
            .map(Source::Art),
    };
    let key = Key {
        source,
        blur: hazy.blur,
        contrast: hazy.contrast,
        saturation: hazy.saturation,
        brightness: hazy.brightness,
    };
    let art = app.backend.art().clone();
    let reduced = app.motion.reduced();
    app.hazy.update(ctx, key, &art, reduced);
    app.hazy.draw(ui, reduced);
}

impl Backdrop {
    fn update(&mut self, ctx: &egui::Context, key: Key, art: &ArtLoader, reduced: bool) {
        let finished = std::mem::take(&mut *self.finished.lock().unwrap_or_else(|p| p.into_inner()));
        if let Some((done, image)) = finished
            .into_iter()
            .rfind(|(done, _)| self.wanted.as_ref() == Some(done))
        {
            self.install(ctx, done, image, reduced);
        }
        if self.wanted.as_ref() != Some(&key) {
            self.wanted = Some(key.clone());
            self.waiting = false;
            match &key.source {
                None => self.install(ctx, key.clone(), None, reduced),
                Some(Source::File(path)) => {
                    let path = path.clone();
                    self.start(ctx, key.clone(), move || std::fs::read(path).ok());
                }
                Some(Source::Art(_)) => self.waiting = true,
            }
        }
        if let (true, Some(Source::Art(url))) = (self.waiting, &key.source) {
            match ctx.try_load_bytes(url) {
                Ok(BytesPoll::Pending { .. }) => {}
                Ok(BytesPoll::Ready { bytes, .. }) => {
                    self.waiting = false;
                    let bytes = bytes.as_ref().to_vec();
                    art.release_bytes(url);
                    self.start(ctx, key, move || Some(bytes));
                }
                Err(_) => {
                    self.waiting = false;
                    self.install(ctx, key, None, reduced);
                }
            }
        }
    }

    fn start(
        &self,
        ctx: &egui::Context,
        key: Key,
        bytes: impl FnOnce() -> Option<Vec<u8>> + Send + 'static,
    ) {
        let (finished, ctx) = (Arc::clone(&self.finished), ctx.clone());
        std::thread::spawn(move || {
            let image = bytes().and_then(|bytes| render(&bytes, &key));
            finished.lock().unwrap_or_else(|p| p.into_inner()).push((key, image));
            ctx.request_repaint();
        });
    }

    fn install(&mut self, ctx: &egui::Context, key: Key, image: Option<ColorImage>, reduced: bool) {
        let fade = !reduced
            && self
                .above
                .as_ref()
                .is_none_or(|above| above.key.source != key.source);
        let texture = image.map(|image| ctx.load_texture("hazy-backdrop", image, TextureOptions::LINEAR));
        let layer = Layer { key, texture };
        if fade {
            self.below = self.above.take();
            self.since = ctx.input(|input| input.time);
        } else {
            self.below = None;
            self.since = f64::NEG_INFINITY;
        }
        self.above = Some(layer);
        ctx.request_repaint();
    }

    fn draw(&self, ui: &egui::Ui, reduced: bool) {
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, BASE);
        let now = ui.ctx().input(|input| input.time);
        let t = if reduced {
            1.0
        } else {
            ((now - self.since) / FADE_SECONDS).clamp(0.0, 1.0)
        };
        if t < 1.0 {
            ui.ctx().request_repaint();
        }
        let eased = (t * t * (3.0 - 2.0 * t)) as f32;
        for (layer, alpha) in [(&self.below, 1.0), (&self.above, eased)] {
            let Some(layer) = layer else { continue };
            match &layer.texture {
                Some(texture) => {
                    let size = texture.size_vec2();
                    painter.image(
                        texture.id(),
                        rect,
                        cover_uv(size.x / size.y, rect),
                        Color32::WHITE.gamma_multiply(alpha),
                    );
                }
                None => {
                    painter.rect_filled(rect, 0.0, BASE.gamma_multiply(alpha));
                }
            }
        }
    }
}

/// The part of a picture that fills `rect`, centred, the rest cropped.
fn cover_uv(aspect: f32, rect: Rect) -> Rect {
    let window = rect.width() / rect.height().max(1.0);
    let (x, y) = if window > aspect {
        (1.0, aspect / window)
    } else {
        (window / aspect, 1.0)
    };
    Rect::from_min_max(pos2((1.0 - x) / 2.0, (1.0 - y) / 2.0), pos2((1.0 + x) / 2.0, (1.0 + y) / 2.0))
}

/// The picture as the backdrop shows it: shrunk, blurred, then contrast,
/// saturation and brightness in CSS's order, laid at 55% over the base
/// colour.
fn render(bytes: &[u8], key: &Key) -> Option<ColorImage> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let (width, height) = (decoded.width().max(1), decoded.height().max(1));
    let scale = SIDE as f32 / width.max(height) as f32;
    let (w, h) = (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    );
    let small = decoded
        .resize_exact(w, h, image::imageops::FilterType::Lanczos3)
        .to_rgb8();
    let (w, h) = (w as usize, h as usize);
    let mut pixels: Vec<[f32; 3]> = small
        .pixels()
        .map(|pixel| pixel.0.map(|channel| f32::from(channel) / 255.0))
        .collect();
    gaussian(&mut pixels, w, h, f32::from(key.blur) * BLUR_SCALE);

    let contrast = f32::from(key.contrast) / 100.0;
    let saturation = f32::from(key.saturation) / 100.0;
    let brightness = f32::from(key.brightness) / 100.0;
    let base = f32::from(BASE.r()) / 255.0;
    let mut out = Vec::with_capacity(w * h * 3);
    for pixel in &pixels {
        let [r, g, b] = pixel.map(|c| ((c - 0.5) * contrast + 0.5).clamp(0.0, 1.0));
        let grey = 0.213 * r + 0.715 * g + 0.072 * b;
        for channel in [r, g, b] {
            let c = (grey + (channel - grey) * saturation).clamp(0.0, 1.0);
            let c = (c * brightness).clamp(0.0, 1.0);
            let c = base + (c - base) * OPACITY;
            out.push((c * 255.0).round() as u8);
        }
    }
    Some(ColorImage::from_rgb([w, h], &out))
}

/// Three box blurs of widths picked to add up to a gaussian of `sigma`.
fn gaussian(pixels: &mut [[f32; 3]], w: usize, h: usize, sigma: f32) {
    if sigma < 0.3 {
        return;
    }
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let wide = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f32 - 12.0 * lower as f32 - 9.0)
        / (-4.0 * lower as f32 - 4.0))
        .round() as i32;
    for pass in 0..3 {
        let size = if pass < wide { lower } else { upper };
        let radius = ((size - 1) / 2) as usize;
        for row in 0..h {
            box_line(pixels, row * w, 1, w, radius);
        }
        for column in 0..w {
            box_line(pixels, column, w, h, radius);
        }
    }
}

/// Averages `len` pixels starting at `start`, `step` apart, over `radius`
/// either side, repeating the end pixels past the edge.
fn box_line(pixels: &mut [[f32; 3]], start: usize, step: usize, len: usize, radius: usize) {
    let line: Vec<[f32; 3]> = (0..len).map(|i| pixels[start + i * step]).collect();
    let span = (2 * radius + 1) as f32;
    let at = |i: isize| line[i.clamp(0, len as isize - 1) as usize];
    let mut sum = [0.0; 3];
    for i in -(radius as isize)..=radius as isize {
        let pixel = at(i);
        (0..3).for_each(|c| sum[c] += pixel[c]);
    }
    for i in 0..len {
        pixels[start + i * step] = sum.map(|c| c / span);
        let (leaving, entering) = (at(i as isize - radius as isize), at(i as isize + radius as isize + 1));
        (0..3).for_each(|c| sum[c] += entering[c] - leaving[c]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(blur: u8) -> Key {
        Key {
            source: None,
            blur,
            contrast: 100,
            saturation: 100,
            brightness: 100,
        }
    }

    #[test]
    fn a_flat_picture_stays_flat_and_sits_under_the_base() {
        let flat = image::RgbImage::from_pixel(300, 300, image::Rgb([200, 100, 50]));
        let mut png = std::io::Cursor::new(Vec::new());
        flat.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let image = render(png.get_ref(), &key(15)).unwrap();
        assert_eq!(image.size, [128, 128]);
        let [r, g, b, _] = image.pixels[64 * 128 + 64].to_array();
        assert!(r.abs_diff(10 + (190.0 * OPACITY) as u8) <= 3, "{r}");
        assert!(g < r && b < g, "{r} {g} {b}");
    }

    #[test]
    fn the_blur_spreads_an_edge() {
        let mut pixels = vec![[0.0; 3]; 16];
        pixels[8] = [1.0; 3];
        gaussian(&mut pixels, 16, 1, 2.0);
        assert!(pixels[8][0] < 0.5 && pixels[6][0] > 0.0);
        let total: f32 = pixels.iter().map(|p| p[0]).sum();
        assert!((total - 1.0).abs() < 0.01, "{total}");
    }

    /// Pressing Next quickly finishes renders out of order: an older one
    /// landing last must not take the place of the one still wanted.
    #[test]
    fn a_late_render_for_an_old_song_cannot_hide_the_wanted_one() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let art = ArtLoader::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            runtime.handle().clone(),
            std::env::temp_dir().join("spotifast-hazy-test-art"),
        );
        let ctx = egui::Context::default();
        let song = |name: &str| Key {
            source: Some(Source::Art(format!("https://i.scdn.co/image/{name}"))),
            ..key(7)
        };
        let mut backdrop = Backdrop {
            wanted: Some(song("wanted")),
            ..Default::default()
        };
        let picture = || Some(ColorImage::filled([2, 2], Color32::RED));
        backdrop
            .finished
            .lock()
            .unwrap()
            .extend([(song("wanted"), picture()), (song("older"), picture())]);
        backdrop.update(&ctx, song("wanted"), &art, true);
        let above = backdrop.above.as_ref().expect("the wanted render is shown");
        assert!(above.key == song("wanted") && above.texture.is_some());
    }

    #[test]
    fn the_cover_crop_keeps_the_centre() {
        let uv = cover_uv(1.0, Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(200.0, 100.0)));
        assert_eq!((uv.min.x, uv.max.x), (0.0, 1.0));
        assert!((uv.height() - 0.5).abs() < 1e-6 && (uv.center().y - 0.5).abs() < 1e-6);
    }
}
