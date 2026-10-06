//! An emoji font of the listener's choosing, such as Apple Color Emoji,
//! kept in the twerkz `emoji` folder. It is memory-mapped, so only the emoji
//! on screen are read from disk. A mapped file cannot be replaced on
//! Windows, so a new choice is written down and swapped in at the next
//! start, before anything maps it.

use std::path::{Path, PathBuf};

const CHOICE: &str = "choice.txt";
const SYSTEM: &str = "system";

pub fn folder() -> PathBuf {
    super::jobs::root(&crate::paths::AppDirs::discover()).join("emoji")
}

fn fonts_in(folder: &Path) -> Vec<PathBuf> {
    let mut fonts: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "ttc"))
                })
                .collect()
        })
        .unwrap_or_default();
    fonts.sort();
    fonts
}

fn choice(folder: &Path) -> Option<String> {
    std::fs::read_to_string(folder.join(CHOICE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The font the app runs with, mapped for its life. Settles a choice made
/// in the last run first: the chosen font stays, the others go.
pub fn user_font() -> Option<&'static [u8]> {
    let folder = folder();
    let _ = std::fs::create_dir_all(&folder);
    let fonts = fonts_in(&folder);
    let path = match choice(&folder).as_deref() {
        Some(SYSTEM) => {
            for font in &fonts {
                let _ = std::fs::remove_file(font);
            }
            let _ = std::fs::remove_file(folder.join(CHOICE));
            return None;
        }
        Some(name) if folder.join(name).is_file() => {
            let chosen = folder.join(name);
            for font in fonts.iter().filter(|font| **font != chosen) {
                let _ = std::fs::remove_file(font);
            }
            chosen
        }
        _ => fonts.into_iter().next()?,
    };
    let file = std::fs::File::open(&path).ok()?;
    // Safety: the font is only read, and the folder is ours.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    log::info!("emoji from {}", path.display());
    Some(&Box::leak(Box::new(map))[..])
}

/// The font the next start will use, or `None` for the system's emoji.
pub fn next_name() -> Option<String> {
    let folder = folder();
    match choice(&folder) {
        Some(name) if name == SYSTEM => None,
        Some(name) if folder.join(&name).is_file() => Some(name),
        _ => fonts_in(&folder)
            .into_iter()
            .next()
            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
    }
}

/// Copies `source` into the emoji folder and chooses it for the next start.
pub fn choose(source: &Path) -> std::io::Result<String> {
    let folder = folder();
    std::fs::create_dir_all(&folder)?;
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "emoji.ttf".to_string());
    let target = folder.join(&name);
    if source != target {
        let partial = target.with_extension("part");
        std::fs::copy(source, &partial)?;
        std::fs::rename(&partial, &target)?;
    }
    std::fs::write(folder.join(CHOICE), &name)?;
    Ok(name)
}

/// Chooses the system's emoji for the next start.
pub fn choose_system() -> std::io::Result<()> {
    let folder = folder();
    std::fs::create_dir_all(&folder)?;
    std::fs::write(folder.join(CHOICE), SYSTEM)
}
