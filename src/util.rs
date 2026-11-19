//! Small helpers shared across the application.

use std::borrow::Cow;

use crate::i18n::{Locale, gettext, ngettext, pgettext};

/// `3:45` for track lengths, `1:02:03` past an hour.
pub fn format_duration_ms(ms: u32) -> String {
    let total = ms / 1000;
    let hours = total / 3600;
    let minutes = (total / 60) % 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// `2 hr 13 min` for playlist totals, `45 min 12 sec` under an hour.
pub fn format_total_ms(locale: Locale, ms: u64) -> String {
    let total = ms / 1000;
    let hours = total / 3600;
    let minutes = (total / 60) % 60;
    let seconds = total % 60;
    if hours > 0 {
        // Translators: A length of time, abbreviated. Keep {hours} and {minutes}.
        gettext(locale, "{hours} hr {minutes} min")
            .replace("{hours}", &hours.to_string())
            .replace("{minutes}", &minutes.to_string())
    } else if minutes > 0 {
        // Translators: A length of time, abbreviated. Keep {minutes} and {seconds}.
        gettext(locale, "{minutes} min {seconds} sec")
            .replace("{minutes}", &minutes.to_string())
            .replace("{seconds}", &seconds.to_string())
    } else {
        // Translators: A length of time, abbreviated. Keep {seconds}.
        gettext(locale, "{seconds} sec").replace("{seconds}", &seconds.to_string())
    }
}

/// Episode lengths read as `1 hr 12 min` or `38 min`.
pub fn format_episode_ms(locale: Locale, ms: u32) -> String {
    let minutes = ms / 60_000;
    let hours = minutes / 60;
    if hours > 0 {
        gettext(locale, "{hours} hr {minutes} min")
            .replace("{hours}", &hours.to_string())
            .replace("{minutes}", &(minutes % 60).to_string())
    } else {
        // Translators: A length of time, abbreviated. Keep {minutes}.
        gettext(locale, "{minutes} min").replace("{minutes}", &minutes.max(1).to_string())
    }
}

pub fn format_count(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out
}

/// `Jan 5, 2024` from an ISO-8601 timestamp or a bare date.
pub fn format_date(locale: Locale, iso: &str) -> String {
    let date = iso.get(..10).unwrap_or(iso);
    let mut parts = date.split('-');
    let (Some(year), Some(month)) = (parts.next(), parts.next()) else {
        return iso.to_string();
    };
    let day = parts.next();
    let month_name = match month {
        "01" => pgettext(locale, "month", "Jan"),
        "02" => pgettext(locale, "month", "Feb"),
        "03" => pgettext(locale, "month", "Mar"),
        "04" => pgettext(locale, "month", "Apr"),
        "05" => pgettext(locale, "month", "May"),
        "06" => pgettext(locale, "month", "Jun"),
        "07" => pgettext(locale, "month", "Jul"),
        "08" => pgettext(locale, "month", "Aug"),
        "09" => pgettext(locale, "month", "Sep"),
        "10" => pgettext(locale, "month", "Oct"),
        "11" => pgettext(locale, "month", "Nov"),
        "12" => pgettext(locale, "month", "Dec"),
        _ => return iso.to_string(),
    };
    let dated = match day.and_then(|day| day.trim_start_matches('0').parse::<u8>().ok()) {
        Some(day) => {
            // Translators: A date. {month} is an abbreviated month name; reorder as your language writes dates.
            gettext(locale, "{month} {day}, {year}").replace("{day}", &day.to_string())
        }
        // Translators: A month and year. {month} is an abbreviated month name.
        None => gettext(locale, "{month} {year}").into_owned(),
    };
    dated
        .replace("{month}", &month_name)
        .replace("{year}", year)
}

/// The window within which timestamps are rendered as relative dates (30 days).
pub const RELATIVE_DATE_LIMIT_SECONDS: i64 = 30 * 24 * 60 * 60;

/// Returns true when `iso` is an ISO timestamp within the relative date window
/// (the past 30 days up to `now`).
pub fn is_relative_date(iso: &str, now: jiff::Timestamp) -> bool {
    let Ok(added) = iso.parse::<jiff::Timestamp>() else {
        return false;
    };
    let seconds = added.duration_until(now).as_secs_f64().floor() as i64;
    (0..RELATIVE_DATE_LIMIT_SECONDS).contains(&seconds)
}

/// `5 minutes ago` for recent ISO-8601 timestamps, otherwise the usual date.
///
/// Dates are shown relatively for their first 30 days, matching the playlist
/// table's compact, time-aware presentation. `now` is an argument so callers
/// can render against one instant and the boundary behaviour stays testable.
pub fn format_relative_date(locale: Locale, iso: &str, now: jiff::Timestamp) -> String {
    let Ok(added) = iso.parse::<jiff::Timestamp>() else {
        return format_date(locale, iso);
    };
    let seconds = added.duration_until(now).as_secs_f64().floor() as i64;
    if !(0..RELATIVE_DATE_LIMIT_SECONDS).contains(&seconds) {
        return format_date(locale, iso);
    }

    let (count, text) = if seconds < 60 {
        let count = seconds;
        let text = ngettext(
            locale,
            // Translators: How long ago a song was added. Keep {count}.
            "{count} second ago",
            "{count} seconds ago",
            count as u32,
        );
        (count, text)
    } else if seconds < 60 * 60 {
        let count = seconds / 60;
        let text = ngettext(
            locale,
            // Translators: How long ago a song was added. Keep {count}.
            "{count} minute ago",
            "{count} minutes ago",
            count as u32,
        );
        (count, text)
    } else if seconds < 24 * 60 * 60 {
        let count = seconds / (60 * 60);
        let text = ngettext(
            locale,
            // Translators: How long ago a song was added. Keep {count}.
            "{count} hour ago",
            "{count} hours ago",
            count as u32,
        );
        (count, text)
    } else if seconds < 7 * 24 * 60 * 60 {
        let count = seconds / (24 * 60 * 60);
        // Translators: How long ago a song was added. Keep {count}.
        let text = ngettext(locale, "{count} day ago", "{count} days ago", count as u32);
        (count, text)
    } else {
        let count = seconds / (7 * 24 * 60 * 60);
        let text = ngettext(
            locale,
            // Translators: How long ago a song was added. Keep {count}.
            "{count} week ago",
            "{count} weeks ago",
            count as u32,
        );
        (count, text)
    };
    text.replace("{count}", &count.to_string())
}

/// Tears the id out of `spotify:track:abc` and friends.
pub fn uri_id(uri: &str) -> Option<&str> {
    uri.rsplit(':').next().filter(|id| !id.is_empty())
}

pub fn uri_kind(uri: &str) -> Option<&str> {
    let mut parts = uri.split(':');
    parts.next()?;
    parts.next()
}

/// Spotify's radio station seeded by a song, playlist, album, or artist.
pub fn station_uri(seed: &str) -> Option<String> {
    let kind = uri_kind(seed)?;
    let id = uri_id(seed)?;
    (seed == format!("spotify:{kind}:{id}")
        && matches!(kind, "track" | "playlist" | "album" | "artist"))
    .then(|| format!("spotify:station:{kind}:{id}"))
}

/// The song, playlist, album, or artist a radio station is seeded by.
pub fn station_seed(station: &str) -> Option<String> {
    let seed = format!("spotify:{}", station.strip_prefix("spotify:station:")?);
    station_uri(&seed).is_some().then_some(seed)
}

pub fn open_spotify_url(uri: &str) -> Option<String> {
    let kind = uri_kind(uri)?;
    let id = uri_id(uri)?;
    Some(format!("https://open.spotify.com/{kind}/{id}"))
}

/// The menu-bar shape for macOS: the disc with the three bars punched
/// out. macOS template images use only the alpha channel and paint the
/// shape themselves, black in a light menu bar and white in a dark one.
pub fn tray_template_rgba(size: usize) -> Vec<u8> {
    static TEMPLATE: std::sync::OnceLock<image::RgbaImage> = std::sync::OnceLock::new();
    let template = TEMPLATE.get_or_init(|| {
        let mut template = mark().clone();
        for pixel in template.pixels_mut() {
            // The bars are the dark colour; make them a hole instead.
            let green = u32::from(pixel[1].saturating_sub(BAR_GREEN)) * 255
                / u32::from(DISC_GREEN - BAR_GREEN);
            pixel.0 = [0, 0, 0, (u32::from(pixel[3]) * green.min(255) / 255) as u8];
        }
        template
    });
    scaled(template, size)
}

/// The mark rasterised to pixels: the window icon, the trays and the logo
/// drawn in the app (`theme::logo`) all use this one picture, the same one
/// the macOS bundle carries.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    scaled(mark(), size)
}

/// The green channel of the bars, below which a pixel is all bar.
const BAR_GREEN: u8 = 40;
/// The green channel of the disc, above which a pixel is all disc.
const DISC_GREEN: u8 = 215;

fn mark() -> &'static image::RgbaImage {
    static MARK: std::sync::OnceLock<image::RgbaImage> = std::sync::OnceLock::new();
    MARK.get_or_init(|| {
        image::load_from_memory(include_bytes!("../packaging/macos/icon-1024.png"))
            .expect("the bundled icon is a PNG")
            .into_rgba8()
    })
}

fn scaled(image: &image::RgbaImage, size: usize) -> Vec<u8> {
    let size = size as u32;
    image::imageops::resize(image, size, size, image::imageops::FilterType::Lanczos3).into_raw()
}

pub fn greeting(locale: Locale) -> Cow<'static, str> {
    match local_hour() {
        5..=11 => gettext(locale, "Good morning"),
        12..=17 => gettext(locale, "Good afternoon"),
        _ => gettext(locale, "Good evening"),
    }
}

fn local_hour() -> u8 {
    jiff::Zoned::now().hour() as u8
}

/// Strips the HTML Spotify embeds in playlist descriptions.
pub fn strip_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for character in text.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }
    decode_entities(&out)
}

/// The text [`strip_html`] gives, with the byte range of each `<a>` link in
/// it and where the link points.
pub fn html_links(text: &str) -> (String, Vec<(std::ops::Range<usize>, String)>) {
    let mut out = String::with_capacity(text.len());
    let mut links = Vec::new();
    let mut open: Option<(usize, String)> = None;
    let mut run = String::new();
    let mut tag: Option<String> = None;
    for character in text.chars() {
        match character {
            '<' => {
                out.push_str(&decode_entities(&std::mem::take(&mut run)));
                tag = Some(String::new());
            }
            '>' => {
                let Some(tag) = tag.take() else { continue };
                if let Some(href) = link_target(&tag) {
                    open = Some((out.len(), href));
                } else if tag.trim().eq_ignore_ascii_case("/a")
                    && let Some((start, href)) = open.take()
                    && start < out.len()
                {
                    links.push((start..out.len(), href));
                }
            }
            _ => match &mut tag {
                Some(tag) => tag.push(character),
                None => run.push(character),
            },
        }
    }
    out.push_str(&decode_entities(&run));
    (out, links)
}

/// Where an opening `<a>` tag points: its `href`, quoted or not. The
/// attributes are read one by one, so a `data-href`, or an `href` inside
/// another attribute's value, is not taken for it.
fn link_target(tag: &str) -> Option<String> {
    let mut rest = tag.strip_prefix(['a', 'A'])?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    loop {
        rest = rest.trim_start();
        let end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        let (name, after) = rest.split_at(end);
        let Some(after) = after.trim_start().strip_prefix('=') else {
            rest = after;
            continue;
        };
        let after = after.trim_start();
        let (value, next) = match after.chars().next() {
            Some(quote @ ('"' | '\'')) => after[1..].split_once(quote)?,
            _ => after.split_once(char::is_whitespace).unwrap_or((after, "")),
        };
        if name.eq_ignore_ascii_case("href") {
            return Some(decode_entities(value)).filter(|href| !href.is_empty());
        }
        rest = next;
    }
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&#x2F;", "/")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The temporary files a replacement (fastframe-file) left in `dir`, which
/// a finished or failed one never does.
#[cfg(test)]
pub(crate) fn temporary_files(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".tmp"))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgba: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let index = (y * size + x) * 4;
        [
            rgba[index],
            rgba[index + 1],
            rgba[index + 2],
            rgba[index + 3],
        ]
    }

    /// The icon is the green disc with dark bars at every size, and the
    /// tray template keeps the disc with the bars cut out.
    #[test]
    fn the_icon_is_the_disc_with_bars_at_every_size() {
        for size in [128, 32] {
            let icon = app_icon_rgba(size);
            assert_eq!(icon.len(), size * size * 4);
            // The disc is green below the bars and the corners are clear.
            let face = pixel(&icon, size, size / 2, size * 7 / 8);
            assert!(face[1] > 150 && face[0] < 100 && face[3] > 240, "{size}: {face:?}");
            assert_eq!(pixel(&icon, size, 1, 1)[3], 0);
            // The top bar crosses the middle, dark.
            let bar = pixel(&icon, size, size / 2, size * 35 / 100);
            assert!(bar[1] < 60 && bar[3] == 255, "{size}: {bar:?}");
        }
        let template = tray_template_rgba(44);
        assert!(pixel(&template, 44, 22, 15)[3] < 40, "the bar is a hole");
        let disc = pixel(&template, 44, 22, 34);
        assert!(disc[..3] == [0, 0, 0] && disc[3] > 240, "{disc:?}");
        assert_eq!(pixel(&template, 44, 1, 1)[3], 0);
    }

    #[test]
    fn radio_stations_map_to_their_seeds_and_back() {
        for kind in ["track", "playlist", "album", "artist"] {
            let seed = format!("spotify:{kind}:4uLU6hMCjMI75M1A2tKUQC");
            let station = station_uri(&seed).expect("a station");
            assert_eq!(
                station,
                format!("spotify:station:{kind}:4uLU6hMCjMI75M1A2tKUQC")
            );
            assert_eq!(station_seed(&station).as_deref(), Some(seed.as_str()));
        }
        for seed in [
            "spotify:show:abc",
            "spotify:episode:abc",
            "spotify:user:me:collection",
            "spotify:track:",
            "track:abc",
        ] {
            assert_eq!(station_uri(seed), None, "{seed}");
        }
        assert_eq!(station_seed("spotify:playlist:abc"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration_ms(225_000), "3:45");
        assert_eq!(format_duration_ms(3_723_000), "1:02:03");
        assert_eq!(format_total_ms(Locale::English, 7_980_000), "2 hr 13 min");
        assert_eq!(format_total_ms(Locale::English, 2_712_000), "45 min 12 sec");
        assert_eq!(format_episode_ms(Locale::English, 4_320_000), "1 hr 12 min");
    }

    #[test]
    fn counts_and_dates() {
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_count(12), "12");
        assert_eq!(
            format_date(Locale::English, "2024-01-05T10:00:00Z"),
            "Jan 5, 2024"
        );
        assert_eq!(format_date(Locale::English, "2024-03"), "Mar 2024");
        assert_eq!(format_date(Locale::English, "2024"), "2024");
    }

    #[test]
    fn recent_dates_are_relative_for_the_first_month() {
        let now: jiff::Timestamp = "2026-08-31T12:00:00Z".parse().unwrap();
        assert_eq!(
            format_relative_date(Locale::English, "2026-08-31T11:59:30Z", now),
            "30 seconds ago"
        );
        assert_eq!(
            format_relative_date(Locale::English, "2026-08-31T11:59:00Z", now),
            "1 minute ago"
        );
        assert_eq!(
            format_relative_date(Locale::English, "2026-08-31T11:00:00Z", now),
            "1 hour ago"
        );
        assert_eq!(
            format_relative_date(Locale::English, "2026-08-30T12:00:00Z", now),
            "1 day ago"
        );
        assert_eq!(
            format_relative_date(Locale::English, "2026-08-17T12:00:00Z", now),
            "2 weeks ago"
        );
        assert_eq!(
            format_relative_date(Locale::English, "2026-08-01T12:00:00Z", now),
            "Aug 1, 2026"
        );
        assert!(is_relative_date("2026-08-31T11:59:30Z", now));
        assert!(is_relative_date("2026-08-31T11:00:00Z", now));
        assert!(is_relative_date("2026-08-30T12:00:00Z", now));
        assert!(is_relative_date("2026-08-17T12:00:00Z", now));
        assert!(!is_relative_date("2026-08-01T12:00:00Z", now));
        assert!(!is_relative_date("2026-09-01T12:00:00Z", now));
        assert!(!is_relative_date("not-a-date", now));

        // Relative date detection is independent of locale wording.
        let pt = format_relative_date(Locale::PortugueseBrazil, "2026-08-31T11:59:30Z", now);
        assert!(!pt.ends_with(" ago"));
        assert!(is_relative_date("2026-08-31T11:59:30Z", now));
    }

    #[test]
    fn dates_and_lengths_follow_the_interface_language() {
        let now: jiff::Timestamp = "2026-08-31T12:00:00Z".parse().unwrap();
        assert_eq!(format_date(Locale::Spanish, "2024-01-05"), "5 ene 2024");
        assert_eq!(format_date(Locale::Spanish, "2024-09"), "sept 2024");
        assert_eq!(format_total_ms(Locale::Spanish, 7_980_000), "2 h 13 min");
        assert_eq!(format_episode_ms(Locale::Spanish, 2_280_000), "38 min");
        for (added, expected) in [
            ("2026-08-31T11:59:59Z", "hace 1 segundo"),
            ("2026-08-31T11:58:00Z", "hace 2 minutos"),
            ("2026-08-30T12:00:00Z", "hace 1 día"),
            ("2026-08-17T12:00:00Z", "hace 2 semanas"),
        ] {
            assert_eq!(format_relative_date(Locale::Spanish, added, now), expected);
        }
        // Each language orders the date its own way.
        assert_eq!(format_date(Locale::Swedish, "2024-01-05"), "5 jan. 2024");
        assert_eq!(format_date(Locale::English, "2024-01-05"), "Jan 5, 2024");
        assert_eq!(
            format_relative_date(Locale::Swedish, "2026-08-30T12:00:00Z", now),
            "för 1 dag sedan"
        );
        assert_eq!(format_date(Locale::Turkish, "2024-01-05"), "5 Oca 2024");
        assert_eq!(format_date(Locale::Turkish, "2024-09"), "Eyl 2024");
        assert_eq!(format_total_ms(Locale::Turkish, 7_980_000), "2 sa 13 dk");
        assert_eq!(format_episode_ms(Locale::Turkish, 2_280_000), "38 dk");
        for (added, expected) in [
            ("2026-08-31T11:59:59Z", "1 saniye önce"),
            ("2026-08-31T11:58:00Z", "2 dakika önce"),
            ("2026-08-30T12:00:00Z", "1 gün önce"),
            ("2026-08-17T12:00:00Z", "2 hafta önce"),
        ] {
            assert_eq!(format_relative_date(Locale::Turkish, added, now), expected);
        }
        assert_eq!(
            format_date(Locale::Ukrainian, "2024-01-05"),
            "5 січ. 2024 р."
        );
        assert_eq!(format_date(Locale::Ukrainian, "2024-09"), "вер. 2024 р.");
        assert_eq!(format_total_ms(Locale::Ukrainian, 7_980_000), "2 год 13 хв");
        assert_eq!(format_episode_ms(Locale::Ukrainian, 2_280_000), "38 хв");
        for (added, expected) in [
            ("2026-08-31T11:59:59Z", "1 секунду тому"),
            ("2026-08-31T11:58:00Z", "2 хвилини тому"),
            ("2026-08-31T07:00:00Z", "5 годин тому"),
            ("2026-08-30T12:00:00Z", "1 день тому"),
            ("2026-08-17T12:00:00Z", "2 тижні тому"),
        ] {
            assert_eq!(
                format_relative_date(Locale::Ukrainian, added, now),
                expected
            );
        }
        assert_eq!(format_date(Locale::Korean, "2024-01-05"), "2024년 1월 5일");
        assert_eq!(format_date(Locale::Korean, "2024-09"), "2024년 9월");
        assert_eq!(format_total_ms(Locale::Korean, 7_980_000), "2시간 13분");
        assert_eq!(format_episode_ms(Locale::Korean, 2_280_000), "38분");
        for (added, expected) in [
            ("2026-08-31T11:59:59Z", "1초 전"),
            ("2026-08-31T11:58:00Z", "2분 전"),
            ("2026-08-31T07:00:00Z", "5시간 전"),
            ("2026-08-30T12:00:00Z", "1일 전"),
            ("2026-08-17T12:00:00Z", "2주 전"),
        ] {
            assert_eq!(format_relative_date(Locale::Korean, added, now), expected);
        }
    }

    #[test]
    fn relative_dates_fall_back_for_future_and_invalid_timestamps() {
        let now: jiff::Timestamp = "2026-08-31T12:00:00Z".parse().unwrap();
        assert_eq!(
            format_relative_date(Locale::English, "2026-09-01T12:00:00Z", now),
            "Sep 1, 2026"
        );
        assert_eq!(
            format_relative_date(Locale::English, "not-a-date", now),
            "not-a-date"
        );
    }

    #[test]
    fn uris() {
        assert_eq!(uri_id("spotify:track:abc"), Some("abc"));
        assert_eq!(uri_kind("spotify:playlist:x"), Some("playlist"));
        assert_eq!(
            open_spotify_url("spotify:album:z").as_deref(),
            Some("https://open.spotify.com/album/z")
        );
    }

    #[test]
    fn html_is_stripped() {
        assert_eq!(
            strip_html("Hi <a href=\"x\">there</a> &amp; you"),
            "Hi there & you"
        );
        assert_eq!(strip_html("ONE&#x2F;TWO&#x2F;THREE"), "ONE/TWO/THREE");
    }

    #[test]
    fn html_links_keep_where_each_link_points() {
        let html = "Mood: <a href=spotify:playlist:pl1>Chill &amp; Co</a>, more at \
                    <a href=\"https://example.com/?a=1&amp;b=2\">example</a>. <b>Enjoy</b>";
        let (text, links) = html_links(html);
        assert_eq!(text, strip_html(html));
        assert_eq!(text, "Mood: Chill & Co, more at example. Enjoy");
        let found: Vec<_> = links
            .iter()
            .map(|(range, href)| (&text[range.clone()], href.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("Chill & Co", "spotify:playlist:pl1"),
                ("example", "https://example.com/?a=1&b=2"),
            ]
        );
        // An anchor without a target, or never closed, is plain text.
        assert!(html_links("<a name=x>x</a> <a href='y'>y").1.is_empty());
        // Only the attribute named href counts, spaced or not.
        let spoofed = "<a data-href=\"https://spoof.example\" title='href=x' \
                       HREF = \"spotify:playlist:pl1\">x</a>";
        assert_eq!(
            html_links(spoofed).1,
            [(0..1, "spotify:playlist:pl1".to_string())]
        );
    }
}
