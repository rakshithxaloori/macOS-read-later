use crate::domain::{Query, Scope};
use crate::reader_style::ReaderFont;
use serde::{Deserialize, Serialize};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub inbox: Query,
    pub archive: Query,
    pub appearance: crate::theme::Appearance,
    pub reader_size: f32,
    pub reader_font: ReaderFont,
    pub window_size: [f32; 2],
    pub inbox_scroll: f32,
    pub archive_scroll: f32,
    pub inbox_loaded: u32,
    pub archive_loaded: u32,
    pub inbox_grouped: bool,
    pub archive_grouped: bool,
    pub window_position: Option<[f32; 2]>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            inbox: Query::for_scope(Scope::Inbox),
            archive: Query::for_scope(Scope::Archive),
            appearance: crate::theme::Appearance::default(),
            reader_size: 19.,
            reader_font: ReaderFont::default(),
            window_size: [1240., 954.],
            inbox_scroll: 0.,
            archive_scroll: 0.,
            inbox_loaded: 60,
            archive_loaded: 60,
            window_position: None,
            inbox_grouped: false,
            archive_grouped: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .map(|mut s: Self| {
                s.reader_size = if s.reader_size.is_finite() {
                    s.reader_size.clamp(15., 27.)
                } else {
                    19.
                };
                for (v, fallback) in s.window_size.iter_mut().zip([1240., 954.]) {
                    if !v.is_finite() || !(600.0..=5000.0).contains(v) {
                        *v = fallback;
                    }
                }
                s.inbox_loaded = s.inbox_loaded.clamp(60, 1_000_000);
                s.archive_loaded = s.archive_loaded.clamp(60, 1_000_000);
                s
            })
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let encoded = serde_json::to_vec_pretty(self)?;
        let root = path.parent().and_then(|p| p.parent());
        let _reservation =
            if let Some(root) = root.filter(|p| p.join("storage-policy.json").is_file()) {
                Some(
                    crate::storage::Storage::new(root, crate::storage::DEFAULT_BUNDLE_BYTES)?
                        .require((encoded.len() as u64) * 2 + 16384)?,
                )
            } else {
                None
            };
        let pending = path.with_extension("pending");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&pending)?;
        f.write_all(&encoded)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        std::fs::rename(pending, path)?;
        std::fs::File::open(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("Settings directory missing"))?,
        )?
        .sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_font_round_trips_without_resetting_existing_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader.json");
        let mut settings = Settings::default();
        settings.inbox.search = "retained search".into();
        settings.reader_size = 23.;
        for font in ReaderFont::ALL {
            settings.reader_font = font;
            settings.save(&path).unwrap();
            let loaded = Settings::load(&path);
            assert_eq!(loaded.reader_font, font);
            assert_eq!(loaded.reader_size, 23.);
            assert_eq!(loaded.inbox.search, "retained search");
        }
    }

    #[test]
    fn old_and_unknown_font_preferences_preserve_the_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader.json");
        for font in [None, Some("unavailable_future_font")] {
            let mut json = serde_json::json!({"reader_size": 24., "inbox_scroll": 143.});
            if let Some(font) = font {
                json["reader_font"] = font.into();
            }
            std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
            let loaded = Settings::load(&path);
            assert_eq!(loaded.reader_font, ReaderFont::Georgia);
            assert_eq!(loaded.reader_size, 24.);
            assert_eq!(loaded.inbox_scroll, 143.);
        }
    }
    #[test]
    fn appearance_round_trip_and_legacy_defaults_preserve_preferences() {
        use crate::theme::Appearance;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for appearance in [Appearance::Light, Appearance::Dark] {
            let settings = Settings {
                appearance,
                reader_size: 23.,
                ..Settings::default()
            };
            settings.save(&path).unwrap();
            let loaded = Settings::load(&path);
            assert_eq!(loaded.appearance, appearance);
            assert_eq!(loaded.reader_size, 23.);
        }
        for value in [
            serde_json::json!({"reader_size": 24.}),
            serde_json::json!({"reader_size": 24., "appearance": "future"}),
        ] {
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            let loaded = Settings::load(&path);
            assert_eq!(loaded.appearance, Appearance::Light);
            assert_eq!(loaded.reader_size, 24.);
        }
    }
}
