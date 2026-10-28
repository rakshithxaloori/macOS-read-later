use serde::{Deserialize, Serialize};

/// Reading preferences are independent of magazine typography and saved content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", from = "String")]
pub enum ReaderFont {
    #[default]
    Georgia,
    Baskerville,
    AvenirNext,
    HelveticaNeue,
}

impl ReaderFont {
    pub const ALL: [Self; 4] = [
        Self::Georgia,
        Self::Baskerville,
        Self::AvenirNext,
        Self::HelveticaNeue,
    ];

    pub fn family(self) -> &'static str {
        match self {
            Self::Georgia => "Georgia",
            Self::Baskerville => "Baskerville",
            Self::AvenirNext => "Avenir Next",
            Self::HelveticaNeue => "Helvetica Neue",
        }
    }
}

impl From<String> for ReaderFont {
    fn from(value: String) -> Self {
        match value.as_str() {
            "baskerville" => Self::Baskerville,
            "avenir_next" => Self::AvenirNext,
            "helvetica_neue" => Self::HelveticaNeue,
            _ => Self::Georgia,
        }
    }
}
