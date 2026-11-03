//! Check GitHub for a newer release and replace this app bundle with it.
//!
//! The latest version comes from the redirect of `/releases/latest`, so no API
//! token or rate-limited JSON endpoint is involved. Every release uploads its
//! disk image under the fixed name `Reader.dmg` (`Read-Later.dmg` before the
//! rename). Installation verifies the mounted app, stages a copy beside the
//! running bundle, and leaves the swap to a small shell helper that runs after
//! this process has exited. The app was called Read Later, so an old bundle is
//! replaced by `Reader.app` in the same folder.
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

pub const REPOSITORY: &str = "rakshithxaloori/macOS-reader";
/// Launch checks run at most once per this interval.
pub const CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;
/// A disk image larger than this is not a Reader release.
const MAX_IMAGE_BYTES: u64 = 60_000_000;
pub const APP_NAME: &str = "Reader.app";
/// Releases before the rename ship this image and bundle name.
const LEGACY_IMAGE: &str = "Read-Later.dmg";
const LEGACY_APP_NAME: &str = "Read Later.app";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);
impl Version {
    /// Accept exactly `X.Y.Z` or `vX.Y.Z`. Pre-release and build suffixes are
    /// not offered as updates.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let mut next = || {
            let part = parts.next()?;
            (!part.is_empty() && part.len() <= 9 && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        };
        let version = Self(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("Cargo version is X.Y.Z")
    }
    pub fn tag(self) -> String {
        format!("v{self}")
    }
}
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Read the release version from the `Location` of `/releases/latest`.
pub fn release_from_location(location: &str) -> Option<Version> {
    let url = url::Url::parse(location)
        .or_else(|_| url::Url::parse("https://github.com")?.join(location))
        .ok()?;
    if url.host_str() != Some("github.com") {
        return None;
    }
    let prefix = format!("/{REPOSITORY}/releases/tag/");
    let path = url.path();
    path.get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(&prefix))
        .and_then(|_| Version::parse(&path[prefix.len()..]))
}

/// The newer release, if `latest` is one.
pub fn newer(latest: Version, current: Version) -> Option<Version> {
    (latest > current).then_some(latest)
}

pub fn image_url(version: Version, name: &str) -> String {
    format!(
        "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
        version.tag()
    )
}
pub fn release_page(version: Version) -> String {
    format!(
        "https://github.com/{REPOSITORY}/releases/tag/{}",
        version.tag()
    )
}

fn client(redirects: bool) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("Reader/", env!("CARGO_PKG_VERSION")))
        .redirect(if redirects {
            reqwest::redirect::Policy::limited(5)
        } else {
            reqwest::redirect::Policy::none()
        })
        .connect_timeout(Duration::from_secs(15))
        .build()?)
}

/// The newest published release.
pub async fn latest() -> Result<Version> {
    let response = tokio::time::timeout(
        Duration::from_secs(25),
        client(false)?
            .head(format!("https://github.com/{REPOSITORY}/releases/latest"))
            .send(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("GitHub did not answer within 25 seconds"))??;
    ensure!(
        response.status().is_redirection(),
        "GitHub answered {} instead of naming the latest release",
        response.status()
    );
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .context("GitHub did not name the latest release")?;
    release_from_location(location)
        .with_context(|| format!("The latest release is not an X.Y.Z version: {location}"))
}

#[derive(Clone, Debug)]
pub enum Event {
    Checked {
        manual: bool,
        result: Result<Version, String>,
    },
    Progress(String),
    /// The verified new bundle is staged beside the running one.
    Staged(Install),
    /// This copy cannot replace itself; the download opened in the browser.
    OpenedDownload(Version),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub version: Version,
    pub bundle: PathBuf,
    pub staged: PathBuf,
}

pub type Sender = std::sync::mpsc::Sender<Event>;

pub fn check(
    runtime: &tokio::runtime::Runtime,
    manual: bool,
    events: Sender,
    wake: impl Fn() + Send + 'static,
) {
    runtime.spawn(async move {
        let result = latest().await.map_err(|e| format!("{e:#}"));
        let _ = events.send(Event::Checked { manual, result });
        wake();
    });
}

/// The `.app` this executable runs from, if it is a bundle at all.
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.parent()?
        .parent()?
        .parent()
        .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

/// Download, verify and stage `version`, or open its download in the browser
/// when this copy cannot replace itself.
pub fn install(
    runtime: &tokio::runtime::Runtime,
    version: Version,
    data: PathBuf,
    events: Sender,
    wake: impl Fn() + Send + Sync + 'static,
) {
    let wake = std::sync::Arc::new(wake);
    let handle = runtime.handle().clone();
    runtime.spawn_blocking(move || {
        let progress = |message: &str| {
            let _ = events.send(Event::Progress(message.into()));
            wake();
        };
        let event = match current_bundle().filter(|b| parent_writable(b)) {
            None => match open_download(version) {
                Ok(()) => Event::OpenedDownload(version),
                Err(e) => Event::Failed(format!("{e:#}")),
            },
            Some(bundle) => match stage(&handle, version, &data, &bundle, &progress) {
                Ok(staged) => Event::Staged(Install {
                    version,
                    bundle,
                    staged,
                }),
                Err(e) => Event::Failed(format!("{e:#}")),
            },
        };
        let _ = events.send(event);
        wake();
    });
}

fn parent_writable(bundle: &Path) -> bool {
    let Some(parent) = bundle.parent() else {
        return false;
    };
    let probe = parent.join(format!(".Reader.update-probe-{}", std::process::id()));
    std::fs::create_dir(&probe).is_ok() && std::fs::remove_dir(&probe).is_ok()
}

fn open_download(version: Version) -> Result<()> {
    run("/usr/bin/open", &[release_page(version).as_str()])
}

pub fn staging_path(bundle: &Path) -> PathBuf {
    bundle.with_file_name(".Reader.update.app")
}
/// Where the new bundle goes: `Reader.app` beside the running one.
pub fn target_path(bundle: &Path) -> PathBuf {
    bundle.with_file_name(APP_NAME)
}

fn stage(
    runtime: &tokio::runtime::Handle,
    version: Version,
    data: &Path,
    bundle: &Path,
    progress: &dyn Fn(&str),
) -> Result<PathBuf> {
    let target = target_path(bundle);
    ensure!(
        target == bundle || !target.exists(),
        "{} already exists next to this app. Remove one of them and try again.",
        target.display()
    );
    let work = data.join("cache").join("update");
    remove(&work)?;
    std::fs::create_dir_all(&work)?;
    let result = (|| {
        progress("Downloading");
        let image = work.join("update.dmg");
        runtime.block_on(download(version, data, &image))?;
        // reqwest never sets quarantine; clear any a policy may have added.
        let _ = run(
            "/usr/bin/xattr",
            &["-dr", "com.apple.quarantine", path(&image)?],
        );
        progress("Verifying");
        let mount = std::env::temp_dir().join(format!("read-later-update-{}", std::process::id()));
        remove(&mount)?;
        std::fs::create_dir_all(&mount)?;
        run(
            "/usr/bin/hdiutil",
            &[
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
                path(&mount)?,
                path(&image)?,
            ],
        )
        .context("The downloaded disk image could not be opened")?;
        let staged = staging_path(bundle);
        let copied = (|| {
            let app = [APP_NAME, LEGACY_APP_NAME]
                .into_iter()
                .map(|name| mount.join(name))
                .find(|app| app.is_dir())
                .context("The disk image does not contain Reader")?;
            verify(&app, version)?;
            remove(&staged)?;
            progress("Copying");
            run("/usr/bin/ditto", &[path(&app)?, path(&staged)?])
        })();
        let _ = run("/usr/bin/hdiutil", &["detach", "-quiet", path(&mount)?]);
        let _ = std::fs::remove_dir(&mount);
        // Free the image's space before the swap briefly holds two bundles.
        let _ = remove(&work);
        copied?;
        let finished = (|| {
            let _ = run(
                "/usr/bin/xattr",
                &["-dr", "com.apple.quarantine", path(&staged)?],
            );
            // A bundle built from this checkout points at its library. The
            // released image is portable, so keep that pointer and re-seal.
            let pointer = Path::new("Contents/Resources/default-data-directory");
            if bundle.join(pointer).is_file() {
                std::fs::copy(bundle.join(pointer), staged.join(pointer))?;
                run(
                    "/usr/bin/codesign",
                    &["--force", "--sign", "-", path(&staged)?],
                )?;
            }
            verify(&staged, version)
        })();
        if finished.is_err() {
            let _ = remove(&staged);
        }
        finished.map(|()| staged)
    })();
    let _ = remove(&work);
    result
}

async fn download(version: Version, data: &Path, destination: &Path) -> Result<()> {
    let client = client(true)?;
    let mut response = None;
    for name in ["Reader.dmg", LEGACY_IMAGE] {
        let attempt = client
            .get(image_url(version, name))
            .timeout(Duration::from_secs(300))
            .send()
            .await?;
        if attempt.status() != reqwest::StatusCode::NOT_FOUND {
            response = Some(attempt.error_for_status()?);
            break;
        }
    }
    let mut response = response.context("The release has no Reader.dmg")?;
    let expected = response.content_length().unwrap_or(MAX_IMAGE_BYTES);
    ensure!(
        expected <= MAX_IMAGE_BYTES,
        "The release image is unexpectedly large"
    );
    let storage = crate::storage::Storage::new(data, crate::storage::DEFAULT_BUNDLE_BYTES)?;
    let reservation = storage.reserve("update download", expected)?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await? {
        written += chunk.len() as u64;
        reservation.check(written)?;
        file.write_all(&chunk)?;
    }
    file.sync_all()?;
    ensure!(
        response_complete(written, expected),
        "The download ended early"
    );
    Ok(())
}

fn response_complete(written: u64, expected: u64) -> bool {
    written > 0 && (expected == MAX_IMAGE_BYTES || written == expected)
}

/// The bundle at `app` is intact and is the release we asked for.
fn verify(app: &Path, version: Version) -> Result<()> {
    ensure!(
        app.join("Contents/MacOS/macos-read-later").is_file(),
        "The disk image does not contain Reader"
    );
    let found = output(
        "/usr/bin/plutil",
        &[
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "-o",
            "-",
            path(&app.join("Contents/Info.plist"))?,
        ],
    )?;
    ensure!(
        Version::parse(found.trim()) == Some(version),
        "The disk image contains version {} instead of {version}",
        found.trim()
    );
    run("/usr/bin/codesign", &["--verify", "--strict", path(app)?])
        .context("The downloaded app's signature is not intact")
}

/// Start the helper that replaces `bundle` once `pid` exits, then relaunches.
pub fn spawn_swap(install: &Install) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let backup = install.bundle.with_file_name(".Reader.previous.app");
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(SWAP_SCRIPT)
        .arg("reader-update")
        .arg(std::process::id().to_string())
        .arg(&install.bundle)
        .arg(&install.staged)
        .arg(backup)
        .arg(target_path(&install.bundle))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("The update helper could not start")?;
    Ok(())
}

/// Wait up to two minutes for the app to quit, then swap by rename so either
/// the old or the new complete bundle is always in place. The new bundle goes
/// to `target`, which differs from `app` when an old Read Later is replaced.
const SWAP_SCRIPT: &str = r#"pid=$1 app=$2 new=$3 old=$4 target=$5
i=0
while kill -0 "$pid" 2>/dev/null; do
  i=$((i + 1))
  [ "$i" -gt 1200 ] && exit 1
  sleep 0.1
done
rm -rf "$old"
if [ "$target" != "$app" ] && [ -e "$target" ]; then
  rm -rf "$new"
  exec /usr/bin/open "$app"
fi
if mv "$app" "$old"; then
  if mv "$new" "$target"; then
    rm -rf "$old"
    exec /usr/bin/open "$target"
  fi
  mv "$old" "$app"
fi
rm -rf "$new"
exec /usr/bin/open "$app"
"#;

fn remove(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => Ok(std::fs::remove_dir_all(path)?),
        Ok(_) => Ok(std::fs::remove_file(path)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn path(p: &Path) -> Result<&str> {
    p.to_str().context("Path is not UTF-8")
}
fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("{program} could not run"))?;
    if !out.status.success() {
        bail!(
            "{} failed: {}",
            Path::new(program)
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
fn run(program: &str, args: &[&str]) -> Result<()> {
    output(program, args).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_only_plain_releases() {
        assert_eq!(Version::parse("v0.3.2"), Some(Version(0, 3, 2)));
        assert_eq!(Version::parse("10.20.30"), Some(Version(10, 20, 30)));
        for garbage in [
            "",
            "v",
            "0.3",
            "0.3.2.1",
            "v0.3.2-beta.1",
            "0.3.2+build",
            "va.b.c",
            "0..2",
            "v 0.3.2",
            "-1.0.0",
            "1.0.0 ",
            "99999999999.0.0",
        ] {
            assert_eq!(Version::parse(garbage), None, "{garbage:?}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn only_strictly_newer_releases_are_offered() {
        let current = Version(0, 3, 2);
        assert_eq!(newer(Version(0, 3, 2), current), None);
        assert_eq!(newer(Version(0, 3, 1), current), None);
        assert_eq!(newer(Version(0, 2, 9), current), None);
        assert_eq!(newer(Version(0, 3, 10), current), Some(Version(0, 3, 10)));
        assert_eq!(newer(Version(0, 4, 0), current), Some(Version(0, 4, 0)));
        assert_eq!(newer(Version(1, 0, 0), current), Some(Version(1, 0, 0)));
    }

    #[test]
    fn release_redirects_name_the_tag() {
        let base = format!("https://github.com/{REPOSITORY}/releases");
        assert_eq!(
            release_from_location(&format!("{base}/tag/v0.3.2")),
            Some(Version(0, 3, 2))
        );
        assert_eq!(
            release_from_location(&format!("/{REPOSITORY}/releases/tag/v1.2.3")),
            Some(Version(1, 2, 3))
        );
        for location in [
            base.clone(),
            format!("{base}/tag/v0.4.0-rc.1"),
            format!("{base}/tag/latest"),
            format!("{base}/tag/v0.4.0/extra"),
            "https://example.com/rakshithxaloori/macOS-reader/releases/tag/v9.0.0".into(),
            "https://github.com/someone/else/releases/tag/v9.0.0".into(),
            "not a url".into(),
        ] {
            assert_eq!(release_from_location(&location), None, "{location}");
        }
    }

    #[test]
    fn image_url_uses_the_fixed_asset_name() {
        assert_eq!(
            image_url(Version(0, 4, 0), "Reader.dmg"),
            "https://github.com/rakshithxaloori/macOS-reader/releases/download/v0.4.0/Reader.dmg"
        );
        assert_eq!(
            target_path(Path::new("/Applications/Read Later.app")),
            Path::new("/Applications/Reader.app")
        );
    }

    #[test]
    fn downloads_must_match_their_declared_length() {
        assert!(response_complete(10, 10));
        assert!(!response_complete(9, 10));
        assert!(!response_complete(0, MAX_IMAGE_BYTES));
        assert!(response_complete(5, MAX_IMAGE_BYTES));
    }

    #[test]
    fn swap_replaces_the_bundle_and_keeps_it_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("Read Later.app");
        let app = target_path(&legacy);
        let new = staging_path(&legacy);
        let old = dir.path().join(".Reader.previous.app");
        // Replace `open` so the test does not launch anything.
        let script = SWAP_SCRIPT.replace("exec /usr/bin/open", "exec /bin/echo");
        let swap = |current: &Path| {
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &script, "sh", "999999999"])
                .args([current, &new, &old, &target_path(current)])
                .output()
                .unwrap();
            assert!(out.status.success());
            PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
        };
        let bundle = |path: &Path, version: &str| {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("v"), version).unwrap();
        };
        // An old Read Later is replaced by Reader.app, which is relaunched.
        bundle(&legacy, "0.3.2");
        bundle(&new, "0.4.0");
        assert_eq!(swap(&legacy), app);
        assert_eq!(std::fs::read_to_string(app.join("v")).unwrap(), "0.4.0");
        assert!(!legacy.exists() && !new.exists() && !old.exists());
        // A later update keeps the name.
        bundle(&new, "0.4.1");
        assert_eq!(swap(&app), app);
        assert_eq!(std::fs::read_to_string(app.join("v")).unwrap(), "0.4.1");
        // A missing staged bundle leaves the current one in place.
        assert_eq!(swap(&app), app);
        assert_eq!(std::fs::read_to_string(app.join("v")).unwrap(), "0.4.1");
        // An unrelated Reader.app is never overwritten.
        bundle(&legacy, "0.3.2");
        bundle(&new, "0.4.2");
        assert_eq!(swap(&legacy), legacy);
        assert_eq!(std::fs::read_to_string(app.join("v")).unwrap(), "0.4.1");
        assert_eq!(std::fs::read_to_string(legacy.join("v")).unwrap(), "0.3.2");
        assert!(!new.exists());
    }
}
