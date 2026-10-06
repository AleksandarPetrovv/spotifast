//! Covers for local songs: a `spotify:local:` URI stands in for the image
//! URL, and this loader answers it with the picture inside the file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;

use crate::localfiles::Index;

enum Cover {
    Pending,
    Ready(Arc<[u8]>),
    Missing,
}

#[derive(Default)]
struct Held {
    index: Option<Arc<Index>>,
    covers: HashMap<String, Cover>,
}

static HELD: LazyLock<Mutex<Held>> = LazyLock::new(Mutex::default);

#[derive(Clone, Copy, Default)]
pub struct LocalCovers;

/// The local files the covers come from. Songs that had none are looked
/// up again in a new index.
pub fn set_index(index: Option<Arc<Index>>) {
    let mut held = HELD.lock().unwrap_or_else(|p| p.into_inner());
    if held.index.as_ref().map(Arc::as_ptr) == index.as_ref().map(Arc::as_ptr) {
        return;
    }
    held.index = index;
    held.covers.retain(|_, cover| matches!(cover, Cover::Ready(_)));
}

fn path_of(index: &Index, uri: &str) -> Option<PathBuf> {
    if let Some(file) = index.files.iter().find(|file| file.uri == uri) {
        return Some(file.path.clone());
    }
    let parts: Vec<&str> = uri.strip_prefix("spotify:local:")?.split(':').collect();
    let [artist, album, title, seconds] = parts.as_slice() else {
        return None;
    };
    let decode = |part: &str| {
        percent_encoding::percent_decode_str(&part.replace('+', " "))
            .decode_utf8_lossy()
            .into_owned()
    };
    index
        .find(
            &decode(artist),
            &decode(album),
            &decode(title),
            Duration::from_secs(seconds.parse().unwrap_or(0)),
        )
        .map(|file| file.path.clone())
}

fn picture(path: &PathBuf) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    let pictures: Vec<_> = tagged.tags().iter().flat_map(|tag| tag.pictures()).collect();
    pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
        .map(|picture| picture.data().to_vec())
}

impl BytesLoader for LocalCovers {
    fn id(&self) -> &'static str {
        "spotifast::LocalCovers"
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !uri.starts_with("spotify:local:") {
            return Err(LoadError::NotSupported);
        }
        let mut held = HELD.lock().unwrap_or_else(|p| p.into_inner());
        match held.covers.get(uri) {
            Some(Cover::Ready(bytes)) => {
                return Ok(BytesPoll::Ready {
                    size: None,
                    bytes: Bytes::Shared(Arc::clone(bytes)),
                    mime: None,
                });
            }
            Some(Cover::Pending) => return Ok(BytesPoll::Pending { size: None }),
            Some(Cover::Missing) => return Err(LoadError::Loading("no cover".into())),
            None => {}
        }
        // Asked again each frame until the first scan is in.
        let Some(index) = held.index.clone() else {
            return Ok(BytesPoll::Pending { size: None });
        };
        held.covers.insert(uri.to_string(), Cover::Pending);
        drop(held);
        let (uri, ctx) = (uri.to_string(), ctx.clone());
        std::thread::spawn(move || {
            let found = path_of(&index, &uri).and_then(|path| picture(&path));
            let mut held = HELD.lock().unwrap_or_else(|p| p.into_inner());
            held.covers.insert(
                uri,
                match found {
                    Some(bytes) => Cover::Ready(bytes.into()),
                    None => Cover::Missing,
                },
            );
            drop(held);
            ctx.request_repaint();
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        HELD.lock().unwrap_or_else(|p| p.into_inner()).covers.remove(uri);
    }

    fn forget_all(&self) {
        HELD.lock().unwrap_or_else(|p| p.into_inner()).covers.clear();
    }

    fn byte_size(&self) -> usize {
        HELD.lock()
            .unwrap_or_else(|p| p.into_inner())
            .covers
            .values()
            .map(|cover| match cover {
                Cover::Ready(bytes) => bytes.len(),
                _ => 0,
            })
            .sum()
    }
}
