use serde::{Deserialize, Serialize};

pub const OWNER: &str = "00000000-0000-4000-8000-000000000001";

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);
        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}
id_type!(ArticleId);
id_type!(FeedId);
id_type!(MagazineId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Inbox,
    Archive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadFilter {
    Unread,
    Read,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    All,
    Saved,
    Rss,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    Saved,
    Published,
    Author,
    Publication,
    Title,
    Duration,
}
impl Sort {
    pub const ALL: [Self; 6] = [
        Self::Saved,
        Self::Published,
        Self::Author,
        Self::Publication,
        Self::Title,
        Self::Duration,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Saved => "Date saved",
            Self::Published => "Publication date",
            Self::Author => "Author",
            Self::Publication => "Publication",
            Self::Title => "Title",
            Self::Duration => "Reading time",
        }
    }
    pub fn sql(self) -> &'static str {
        match self {
            Self::Saved => "saved_at",
            Self::Published => "published_at",
            Self::Author => "NULLIF(lower(trim(author)), '')",
            Self::Publication => "NULLIF(lower(trim(site)), '')",
            Self::Title => "NULLIF(lower(trim(title)), '')",
            Self::Duration => "NULLIF(word_count, 0)",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub scope: Scope,
    pub read: ReadFilter,
    pub origin: Origin,
    pub sort: Sort,
    pub ascending: bool,
    pub search: String,
    pub publication: String,
}
impl Query {
    pub fn for_scope(scope: Scope) -> Self {
        Self {
            scope,
            read: if scope == Scope::Inbox {
                ReadFilter::Unread
            } else {
                ReadFilter::Read
            },
            origin: Origin::All,
            sort: Sort::Saved,
            ascending: false,
            search: String::new(),
            publication: String::new(),
        }
    }
    pub fn sort_by(&mut self, sort: Sort) {
        if self.sort == sort {
            self.ascending = !self.ascending
        } else {
            self.sort = sort;
            self.ascending = false;
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CopyStatus {
    Pending,
    Saved,
    Partial,
    Failed,
}
impl CopyStatus {
    pub fn parse(v: &str) -> Result<Self, String> {
        match v {
            "pending" => Ok(Self::Pending),
            "saved" => Ok(Self::Saved),
            "partial" => Ok(Self::Partial),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid copy status {v}")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Saved => "saved",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Article {
    pub id: ArticleId,
    pub url: String,
    pub title: String,
    pub author: Option<String>,
    pub site: String,
    pub logo_url: Option<String>,
    pub excerpt: String,
    pub word_count: u32,
    pub status: CopyStatus,
    pub error: Option<String>,
    pub published_at: Option<String>,
    pub saved_at: String,
    pub read_at: Option<String>,
    pub archived_at: Option<String>,
    pub deliberate: bool,
    pub excluded: bool,
    pub printed: u32,
    pub drafted: u32,
}
impl Article {
    pub fn minutes(&self) -> u32 {
        self.word_count.div_ceil(230)
    }
    pub fn reading_label(&self) -> String {
        if self.word_count == 0 {
            "Reading time unknown".into()
        } else {
            format!("{} min read", self.minutes())
        }
    }
}
#[derive(Clone, Debug)]
pub struct ArticleBody {
    pub article: Article,
    pub html: String,
    pub media: Vec<(String, Vec<u8>)>,
}
#[derive(Clone, Debug)]
pub struct Feed {
    pub id: FeedId,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub last_polled_at: Option<String>,
    pub error: Option<String>,
    pub activity: FeedActivity,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeedActivity {
    #[default]
    Idle,
    Queued,
    Running,
}
#[derive(Clone, Debug)]
pub struct Magazine {
    pub id: MagazineId,
    pub title: String,
    pub subtitle: String,
    pub created_at: String,
    pub finalized_at: Option<String>,
    pub issue_number: u32,
    pub paper_size: crate::magazine::PaperSize,
    pub page_count: Option<u32>,
}
#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub inbox: u32,
    pub unread: u32,
    pub archive: u32,
    pub all: u32,
}
#[derive(Clone, Copy, Debug)]
pub enum FeedAction {
    Pause,
    Resume,
    Sync,
    Remove,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrivalKind {
    Web,
    Rss,
    WhatsApp,
    Import,
}
impl ArrivalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Rss => "rss",
            Self::WhatsApp => "whatsapp",
            Self::Import => "web",
        }
    }
}

pub fn date_label(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|_| value.chars().take(10).collect())
}
