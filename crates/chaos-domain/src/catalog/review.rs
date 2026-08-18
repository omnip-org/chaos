use uuid::Uuid;

use crate::{DomainError, FieldViolation, identity::Email};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReviewId(Uuid);

impl ReviewId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ReviewId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewRating(u8);

impl ReviewRating {
    pub fn parse(value: u8) -> Result<Self, DomainError> {
        if (1..=5).contains(&value) {
            Ok(Self(value))
        } else {
            Err(validation("rating", "must be between 1 and 5"))
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// A customer-submitted, top-level review. Always carries a rating; never a parent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewContent {
    rating: ReviewRating,
    title: Option<String>,
    content: String,
    author_name: String,
    author_email: Option<Email>,
}

impl ReviewContent {
    pub fn new(
        rating: ReviewRating,
        title: Option<String>,
        content: impl Into<String>,
        author_name: impl Into<String>,
        author_email: Option<Email>,
    ) -> Result<Self, DomainError> {
        let content = content.into();
        let author_name = author_name.into();
        if content.trim().is_empty() || content.chars().count() > 10_000 {
            return Err(validation("content", "must contain 1-10000 characters"));
        }
        if author_name.trim().is_empty() || author_name.chars().count() > 120 {
            return Err(validation("author_name", "must contain 1-120 characters"));
        }
        let title = title
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let Some(title) = &title
            && title.chars().count() > 255
        {
            return Err(validation("title", "must contain at most 255 characters"));
        }
        Ok(Self {
            rating,
            title,
            content,
            author_name,
            author_email,
        })
    }

    pub const fn rating(&self) -> ReviewRating {
        self.rating
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn author_name(&self) -> &str {
        &self.author_name
    }

    pub fn author_email(&self) -> Option<&Email> {
        self.author_email.as_ref()
    }
}

/// A staff-authored reply to an existing review. Never carries a rating and is
/// always created already approved — staff-authored content needs no moderation
/// of itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaffReplyContent(String);

impl StaffReplyContent {
    pub fn new(content: impl Into<String>) -> Result<Self, DomainError> {
        let content = content.into();
        if content.trim().is_empty() || content.chars().count() > 10_000 {
            return Err(validation("content", "must contain 1-10000 characters"));
        }
        Ok(Self(content))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
}

impl ReviewStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

fn validation(field: &'static str, reason: &str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_must_be_one_through_five() {
        assert!(ReviewRating::parse(0).is_err());
        assert!(ReviewRating::parse(6).is_err());
        assert!(ReviewRating::parse(1).is_ok());
        assert!(ReviewRating::parse(5).is_ok());
    }

    #[test]
    fn review_content_requires_non_empty_bounded_content_and_author_name() {
        let rating = ReviewRating::parse(4).unwrap();
        assert!(ReviewContent::new(rating, None, "", "Jane", None).is_err());
        assert!(ReviewContent::new(rating, None, "Great poles", "", None).is_err());
        assert!(ReviewContent::new(rating, None, "x".repeat(10_001), "Jane", None).is_err());
        assert!(ReviewContent::new(rating, None, "Great poles", "Jane", None).is_ok());
    }

    #[test]
    fn review_content_treats_blank_title_as_absent() {
        let rating = ReviewRating::parse(5).unwrap();
        let content = ReviewContent::new(
            rating,
            Some("   ".into()),
            "Solid build quality.",
            "Jane",
            None,
        )
        .unwrap();
        assert_eq!(content.title(), None);
    }

    #[test]
    fn staff_reply_shares_content_bounds_but_carries_no_rating() {
        assert!(StaffReplyContent::new("").is_err());
        assert!(StaffReplyContent::new("Thanks for the feedback!").is_ok());
    }

    #[test]
    fn review_status_round_trips_through_as_str_and_parse() {
        for status in [
            ReviewStatus::Pending,
            ReviewStatus::Approved,
            ReviewStatus::Rejected,
        ] {
            assert_eq!(ReviewStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(ReviewStatus::parse("unknown"), None);
    }
}
