pub mod extraction;
#[path = "../../../src/native_pdf.rs"]
pub mod native_pdf;
#[cfg(target_os = "macos")]
pub mod native_text;
#[path = "../../../src/reader_style.rs"]
pub mod reader_style;
#[path = "../../../src/rich_content.rs"]
pub mod rich_content;
pub mod domain {
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct ArticleId(pub String);
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct MagazineId(pub String);
    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all="lowercase")]
    pub enum CopyStatus {
        Pending,
        Saved,
        Partial,
        Failed,
    }
}
#[path = "../../../src/extraction.rs"]
pub mod app_extraction;

#[path = "../../../src/production_renderer.rs"]
pub mod production_renderer;
pub mod magazine;

#[path = "../../../src/theme.rs"]
pub mod theme;
