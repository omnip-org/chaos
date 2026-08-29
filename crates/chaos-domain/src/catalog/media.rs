use uuid::Uuid;

use crate::{DomainError, FieldViolation};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MediaAssetId(Uuid);

impl MediaAssetId {
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

impl Default for MediaAssetId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaAssetStatus {
    PendingUpload,
    Ready,
    Archived,
}

impl MediaAssetStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingUpload => "pending_upload",
            Self::Ready => "ready",
            Self::Archived => "archived",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending_upload" => Some(Self::PendingUpload),
            "ready" => Some(Self::Ready),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaDescriptor {
    file_name: String,
    media_type: String,
    kind: MediaKind,
    byte_size: u64,
    sha256_hex: String,
    alt_text: String,
}

impl MediaDescriptor {
    pub fn new(
        file_name: impl Into<String>,
        media_type: impl Into<String>,
        byte_size: u64,
        sha256_hex: impl Into<String>,
        alt_text: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let file_name = file_name.into();
        if file_name.trim().is_empty()
            || file_name.chars().count() > 255
            || file_name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(validation(
                "file_name",
                "must contain 1-255 non-control characters without path separators",
            ));
        }
        let media_type = media_type.into().to_ascii_lowercase();
        let kind = match media_type.as_str() {
            "image/jpeg" | "image/png" | "image/webp" | "image/avif" | "image/gif" => {
                MediaKind::Image
            }
            "video/mp4" | "video/webm" => MediaKind::Video,
            _ => {
                return Err(validation(
                    "media_type",
                    "must be an allowlisted image or video media type",
                ));
            }
        };
        let maximum = match kind {
            MediaKind::Image => 25 * 1024 * 1024,
            MediaKind::Video => 500 * 1024 * 1024,
        };
        if byte_size == 0 || byte_size > maximum {
            return Err(validation(
                "byte_size",
                match kind {
                    MediaKind::Image => "must be 1-26214400 bytes for images",
                    MediaKind::Video => "must be 1-524288000 bytes for videos",
                },
            ));
        }
        let sha256_hex = sha256_hex.into();
        if sha256_hex.len() != 64
            || !sha256_hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(validation(
                "sha256",
                "must be 64 lowercase hexadecimal characters",
            ));
        }
        let alt_text = alt_text.into();
        if alt_text.chars().count() > 500 || alt_text.chars().any(char::is_control) {
            return Err(validation(
                "alt_text",
                "must contain at most 500 non-control characters",
            ));
        }
        Ok(Self {
            file_name,
            media_type,
            kind,
            byte_size,
            sha256_hex,
            alt_text,
        })
    }
    pub fn file_name(&self) -> &str {
        &self.file_name
    }
    pub fn media_type(&self) -> &str {
        &self.media_type
    }
    pub const fn kind(&self) -> MediaKind {
        self.kind
    }
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }
    pub fn sha256_hex(&self) -> &str {
        &self.sha256_hex
    }
    pub fn alt_text(&self) -> &str {
        &self.alt_text
    }
}

fn validation(field: &'static str, reason: &'static str) -> DomainError {
    DomainError::Validation(vec![FieldViolation {
        field,
        reason: reason.into(),
    }])
}

#[cfg(test)]
mod tests {
    use super::MediaDescriptor;
    #[test]
    fn media_descriptor_allowlists_type_size_and_digest() {
        let digest = "a".repeat(64);
        assert!(
            MediaDescriptor::new("front.webp", "image/webp", 1024, &digest, "Front view").is_ok()
        );
        assert!(MediaDescriptor::new("../secret", "image/webp", 1024, &digest, "").is_err());
        assert!(MediaDescriptor::new("front.svg", "image/svg+xml", 1024, &digest, "").is_err());
        assert!(MediaDescriptor::new("movie.mp4", "video/mp4", 524_288_001, &digest, "").is_err());
        assert!(MediaDescriptor::new("front.webp", "image/webp", 1024, "ABC", "").is_err());
    }
}
