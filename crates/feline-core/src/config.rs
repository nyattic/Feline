use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    #[default]
    E621,
    E926,
}

impl Site {
    pub fn host(&self) -> &'static str {
        match self {
            Site::E621 => "e621.net",
            Site::E926 => "e926.net",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RatingFilter {
    pub safe: bool,
    pub questionable: bool,
    pub explicit: bool,
}

impl RatingFilter {
    pub fn all() -> Self {
        Self {
            safe: true,
            questionable: true,
            explicit: true,
        }
    }

    /// Returns e621 rating filter tokens to append to a search query.
    /// If all three are enabled or all three are disabled, no filter is applied.
    pub fn as_query_fragment(&self) -> Option<String> {
        let selected: Vec<&'static str> = [
            self.safe.then_some("s"),
            self.questionable.then_some("q"),
            self.explicit.then_some("e"),
        ]
        .into_iter()
        .flatten()
        .collect();

        if selected.is_empty() || selected.len() == 3 {
            return None;
        }

        if selected.len() == 1 {
            Some(format!("rating:{}", selected[0]))
        } else {
            Some(format!("rating:{}", selected.join(",")))
        }
    }
}

/// Optional `-type:...` filters injected into every search.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MediaSkip {
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub flash: bool,
    #[serde(default)]
    pub animation: bool,
}

impl MediaSkip {
    /// Returns the negated `-type:` tokens for the enabled skip flags.
    pub fn as_query_tokens(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.video {
            out.push("-type:webm");
        }
        if self.flash {
            out.push("-type:swf");
        }
        if self.animation {
            out.push("-type:gif");
        }
        out
    }
}
