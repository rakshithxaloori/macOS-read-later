use crate::domain::{ArticleId, CopyStatus, MagazineId};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperSize {
    #[default]
    Letter,
    A4,
    A5,
}
impl PaperSize {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Letter => "Letter",
            Self::A4 => "A4",
            Self::A5 => "A5",
        }
    }
    pub fn points(self) -> (f64, f64) {
        match self {
            Self::Letter => (612., 792.),
            Self::A4 => (595.28, 841.89),
            Self::A5 => (419.53, 595.28),
        }
    }
    pub fn margin(self) -> f64 {
        if self == Self::A5 { 32. } else { 46. }
    }
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "Letter" => Ok(Self::Letter),
            "A4" => Ok(Self::A4),
            "A5" => Ok(Self::A5),
            _ => bail!("Invalid paper size"),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Font {
    #[default]
    Georgia,
    Baskerville,
    Avenir,
    Helvetica,
}
impl Font {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Georgia => "Georgia",
            Self::Baskerville => "Baskerville",
            Self::Avenir => "Avenir",
            Self::Helvetica => "Helvetica",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Columns {
    #[default]
    Auto,
    One,
    Two,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrintProfile {
    #[default]
    #[serde(rename = "home")]
    Home,
    #[serde(rename = "canva-us-large-magazine")]
    Canva,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Theme {
    pub name: String,
    pub display_font: Font,
    pub body_font: Font,
    pub paper: String,
    pub ink: String,
    pub accent: String,
    pub muted: String,
}
impl Default for Theme {
    fn default() -> Self {
        themes().remove(0)
    }
}
pub fn themes() -> Vec<Theme> {
    [
        (
            "Signal & Paper",
            Font::Georgia,
            Font::Georgia,
            "#f2eee4",
            "#1d211d",
            "#d64b2a",
            "#667067",
        ),
        (
            "Literary Review",
            Font::Baskerville,
            Font::Baskerville,
            "#f5f0e6",
            "#251c18",
            "#7a2635",
            "#6b5b52",
        ),
        (
            "Swiss Journal",
            Font::Helvetica,
            Font::Avenir,
            "#f4f3ef",
            "#111111",
            "#ef3f2f",
            "#656565",
        ),
        (
            "Modern Review",
            Font::Avenir,
            Font::Helvetica,
            "#f1f0ea",
            "#172027",
            "#236b78",
            "#617078",
        ),
    ]
    .into_iter()
    .map(
        |(name, display_font, body_font, paper, ink, accent, muted)| Theme {
            name: name.into(),
            display_font,
            body_font,
            paper: paper.into(),
            ink: ink.into(),
            accent: accent.into(),
            muted: muted.into(),
        },
    )
    .collect()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Design {
    pub theme: Theme,
    pub body_size_pt: f64,
    pub line_height: f64,
    pub columns: Columns,
    pub column_gap_pt: f64,
    pub paragraph_space_pt: f64,
    pub profile: PrintProfile,
    pub cover_text_corner: String,
    pub cover_essay_titles: bool,
    pub front_prompt: String,
    pub back_prompt: String,
    pub front_asset: Option<String>,
    pub back_asset: Option<String>,
    pub companion_asset: Option<String>,
}
impl Default for Design {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            body_size_pt: 9.5,
            line_height: 1.22,
            columns: Columns::Auto,
            column_gap_pt: 27.36,
            paragraph_space_pt: 8.,
            profile: PrintProfile::Home,
            cover_text_corner: "top-left".into(),
            cover_essay_titles: true,
            front_prompt: "japanese-watercolor-v5".into(),
            back_prompt: "japanese-watercolor-back-v2".into(),
            front_asset: None,
            back_asset: None,
            companion_asset: None,
        }
    }
}
impl Design {
    pub fn column_count(&self, paper: PaperSize) -> usize {
        match self.columns {
            Columns::One => 1,
            Columns::Two => 2,
            Columns::Auto => {
                if paper == PaperSize::A5 {
                    1
                } else {
                    2
                }
            }
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PublishedDate {
    pub display: String,
    pub source_url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInsertion {
    pub asset_id: String,
    pub sha256: String,
    pub after_text: String,
    pub alt: String,
    pub note: String,
    pub source_url: String,
    pub image_url: String,
    pub retrieved_at: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Presentation {
    pub display_author: Option<String>,
    pub author_source: Option<String>,
    pub published_date: Option<PublishedDate>,
    pub image_layouts: BTreeMap<String, String>,
    pub insertions: Vec<ImageInsertion>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub article_id: ArticleId,
    pub position: u32,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub html: String,
    pub presented_html: String,
    pub word_count: u32,
    pub content_status: CopyStatus,
    pub content_warning: Option<String>,
    pub presentation: Presentation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub id: MagazineId,
    pub title: String,
    pub subtitle: String,
    pub paper_size: PaperSize,
    pub design: Design,
    pub created_at: String,
    pub finalized_at: Option<String>,
    pub issue_number: u32,
    pub articles: Vec<Snapshot>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderJob {
    pub issue: Issue,
    pub media: HashMap<String, Vec<u8>>,
    pub input_sha256: String,
}
