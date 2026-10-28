pub mod controls;
pub mod dialog;
pub mod library;
pub mod rail;
pub mod reader;
pub mod secondary;

use crate::{backend::Command, domain::*};

/// The transparent title bar overlaps the top of every pane. Keep it free of
/// controls so the window stays draggable and the traffic lights stay clear.
pub const TITLEBAR_INSET: f32 = 28.;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Inbox,
    Archive,
    Feeds,
    Magazines,
    WhatsApp,
    ImportExport,
}
impl View {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Archive => "Archive",
            Self::Feeds => "Your feeds",
            Self::Magazines => "Magazines",
            Self::WhatsApp => "WhatsApp",
            Self::ImportExport => "Import / export",
        }
    }
}
#[derive(Clone, Debug)]
pub enum Action {
    Navigate(View),
    Open(ArticleId),
    CloseReader,
    Query(Query),
    LoadMore,
    Command(Command),
    ChooseImport,
    ChooseExport,
    SaveDialog,
    FeedDialog,
    SelectMagazine,
    SelectDelete,
    CancelSelection,
    ShowDetails { title: String, message: String },
    ToggleSelection(ArticleId),
    ConfirmDelete(Vec<ArticleId>),
    ConfirmDeleteMatching { query: Query, expected: u32 },
    Group(bool),
    DraftDialog,
    FontSize(f32),
    ReaderFont(crate::reader_style::ReaderFont),
    Appearance(crate::theme::Appearance),
    WhatsAppConnect(String),
    WhatsAppDisconnect,
    Quit,
}

pub mod icons;
pub mod magazine;

pub mod import_export;
pub mod whatsapp;
