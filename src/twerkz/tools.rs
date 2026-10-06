//! yt-dlp, ffmpeg and deno: whatever already runs from the command line,
//! else fetched once into the twerkz tools folder. Our own yt-dlp updates
//! itself weekly, since YouTube breaks old versions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug)]
pub struct Tools {
    pub ytdlp: PathBuf,
    pub ffmpeg: PathBuf,
    pub deno: Option<PathBuf>,
}

impl Tools {
    pub fn ffmpeg_dir(&self) -> PathBuf {
        self.ffmpeg.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    /// The arguments every yt-dlp call starts with.
    pub fn ytdlp_base(&self) -> Vec<String> {
        let mut args = vec![
            "--ignore-config".to_string(),
            "--no-warnings".to_string(),
            "--socket-timeout".to_string(),
            "20".to_string(),
            "--ffmpeg-location".to_string(),
            self.ffmpeg_dir().to_string_lossy().into_owned(),
        ];
        if let Some(deno) = &self.deno {
            args.push("--js-runtimes".to_string());
            args.push(format!("deno:{}", deno.to_string_lossy()));
        }
        args
    }
}

static SETUP: tokio::sync::Mutex<Option<Tools>> = tokio::sync::Mutex::const_new(None);

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// The tools, set up on first use. `status` hears what is being fetched.
pub async fn ensure(http: &reqwest::Client, dir: &Path, status: &(dyn Fn(String) + Sync)) -> Result<Tools> {
    let mut held = SETUP.lock().await;
    if let Some(tools) = held.as_ref() {
        return Ok(tools.clone());
    }
    // A tool that already works from the command line is used as it is;
    // only what is missing is fetched into our folder.
    let (on_path_ytdlp, on_path_ffmpeg, on_path_deno) = tokio::join!(
        usable("yt-dlp", "--version"),
        usable("ffmpeg", "-version"),
        usable("deno", "--version"),
    );
    let ytdlp = match on_path_ytdlp {
        Some(path) => path,
        None => {
            let own = dir.join(format!("yt-dlp{EXE}"));
            if !own.is_file() {
                status("Downloading yt-dlp…".to_string());
                download(http, ytdlp_url()?, &own).await?;
                make_executable(&own)?;
            } else if stale(&own) {
                self_update(&own).await;
            }
            own
        }
    };
    let ffmpeg = match on_path_ffmpeg {
        Some(path) => path,
        None => {
            let own = dir.join("ffmpeg").join(format!("ffmpeg{EXE}"));
            if !own.is_file() {
                status("Downloading ffmpeg…".to_string());
                fetch_ffmpeg(http, dir, &own).await?;
            }
            own
        }
    };
    let deno = match on_path_deno {
        Some(path) => path,
        None => {
            let own = dir.join(format!("deno{EXE}"));
            if !own.is_file() {
                status("Downloading deno…".to_string());
                let url = deno_url().context("no deno build for this platform")?;
                let archive = dir.join("deno.zip");
                download(http, url, &archive).await?;
                unzip_one(&archive, &format!("deno{EXE}"), &own)?;
                let _ = std::fs::remove_file(&archive);
                make_executable(&own)?;
            }
            own
        }
    };
    let tools = Tools {
        ytdlp,
        ffmpeg,
        deno: Some(deno),
    };
    log::info!("download tools: {tools:?}");
    *held = Some(tools.clone());
    Ok(tools)
}

/// `name` on PATH, if it runs.
async fn usable(name: &str, version_flag: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let path = std::env::split_paths(&paths)
        .map(|dir| dir.join(format!("{name}{EXE}")))
        .find(|path| path.is_file())?;
    let mut check = command(&path);
    check
        .arg(version_flag)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let ran = tokio::time::timeout(Duration::from_secs(10), check.status()).await;
    matches!(ran, Ok(Ok(status)) if status.success()).then_some(path)
}

fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > Duration::from_secs(7 * 24 * 60 * 60))
}

async fn self_update(path: &Path) {
    let mut command = command(path);
    command.arg("-U");
    match tokio::time::timeout(Duration::from_secs(60), command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            if let Ok(file) = std::fs::File::options().append(true).open(path) {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
        }
        _ => log::warn!("yt-dlp could not update itself"),
    }
}

/// A process with no console window.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
}

fn ytdlp_url() -> Result<&'static str> {
    const BASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/";
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => concat!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/", "yt-dlp.exe"),
        ("macos", _) => concat!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/", "yt-dlp_macos"),
        ("linux", "aarch64") => concat!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/", "yt-dlp_linux_aarch64"),
        ("linux", _) => concat!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/", "yt-dlp_linux"),
        _ => bail!("no yt-dlp build for this platform at {BASE}"),
    })
}

fn deno_url() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip",
        ("macos", "aarch64") => "https://github.com/denoland/deno/releases/latest/download/deno-aarch64-apple-darwin.zip",
        ("macos", _) => "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-apple-darwin.zip",
        ("linux", "aarch64") => "https://github.com/denoland/deno/releases/latest/download/deno-aarch64-unknown-linux-gnu.zip",
        ("linux", _) => "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip",
        _ => return None,
    })
}

async fn fetch_ffmpeg(http: &reqwest::Client, dir: &Path, target: &Path) -> Result<()> {
    let folder = target.parent().context("bad ffmpeg path")?;
    std::fs::create_dir_all(folder)?;
    let archive = dir.join("ffmpeg.zip");
    match std::env::consts::OS {
        "windows" => {
            download(
                http,
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
                &archive,
            )
            .await?;
            unzip_one(&archive, "ffprobe.exe", &folder.join("ffprobe.exe"))?;
            unzip_one(&archive, "ffmpeg.exe", target)?;
        }
        "macos" => {
            download(http, "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip", &archive).await?;
            unzip_one(&archive, "ffprobe", &folder.join("ffprobe"))?;
            make_executable(&folder.join("ffprobe"))?;
            download(http, "https://evermeet.cx/ffmpeg/getrelease/zip", &archive).await?;
            unzip_one(&archive, "ffmpeg", target)?;
            make_executable(target)?;
        }
        _ => bail!("install ffmpeg with your package manager to download songs"),
    }
    let _ = std::fs::remove_file(&archive);
    Ok(())
}

pub async fn download(http: &reqwest::Client, url: &str, target: &Path) -> Result<()> {
    let mut response = http
        .get(url)
        .timeout(Duration::from_secs(15 * 60))
        .send()
        .await
        .with_context(|| format!("cannot reach {url}"))?
        .error_for_status()
        .with_context(|| format!("cannot download {url}"))?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.context("cannot create the tools folder")?;
    }
    let partial = target.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await?;
    while let Some(chunk) = response.chunk().await.context("download interrupted")? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    tokio::fs::rename(&partial, target).await?;
    Ok(())
}

/// Extracts the first entry named `name`, whatever folder it sits in.
fn unzip_one(archive: &Path, name: &str, target: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).context("broken archive")?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let matches = Path::new(entry.name())
            .file_name()
            .is_some_and(|file| file == name);
        if matches && entry.is_file() {
            let partial = target.with_extension("part");
            let mut out = std::fs::File::create(&partial)?;
            std::io::copy(&mut entry, &mut out)?;
            drop(out);
            std::fs::rename(&partial, target)?;
            return Ok(());
        }
    }
    bail!("{name} is missing from the archive")
}

/// Extracts every file under `prefix` in the archive into `target`, swapped
/// in whole so a half-written folder is never used.
pub fn unzip_folder(archive: &Path, prefix: &str, target: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).context("broken archive")?;
    let partial = target.with_extension("part");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let Ok(relative) = name.strip_prefix(prefix.trim_end_matches('/')) else {
            continue;
        };
        if !entry.is_file() || relative.as_os_str().is_empty() {
            continue;
        }
        let out_path = partial.join(relative);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(&partial, target)?;
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod live {
    #[tokio::test]
    #[ignore = "network, large download"]
    async fn ffmpeg_download_unpacks() {
        let dir = std::env::temp_dir().join("twerkz-ffmpeg-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("ffmpeg").join(format!("ffmpeg{}", super::EXE));
        super::fetch_ffmpeg(&reqwest::Client::new(), &dir, &target).await.unwrap();
        let out = std::process::Command::new(&target).arg("-version").output().unwrap();
        eprintln!("{}", String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or(""));
        assert!(out.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
