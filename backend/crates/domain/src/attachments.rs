//! Attachments of tickets (specification §12, L05-US05): what may be
//! uploaded, how a file goes from its upload to its download, who removes
//! it.
//!
//! A file is uploaded straight to the storage, then scanned by the
//! antivirus: only a clean file can be downloaded. Its SHA-256, computed
//! while scanning, deduplicates it within the tenant: a second upload of
//! the same content points to the first object and its own is removed.

/// Where an attachment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waiting for the browser to upload it.
    Pending,
    /// Uploaded, being scanned.
    Scanning,
    /// Scanned, nothing found: downloadable.
    Clean,
    /// A virus was found: its content is deleted, it is never downloadable.
    Infected,
    /// Not what was announced (another size), or unreadable: deleted.
    Rejected,
}

impl Status {
    pub const ALL: [Self; 5] = [
        Self::Pending,
        Self::Scanning,
        Self::Clean,
        Self::Infected,
        Self::Rejected,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Scanning => "scanning",
            Self::Clean => "clean",
            Self::Infected => "infected",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// Only a scanned, clean file is ever downloaded.
    pub const fn downloadable(self) -> bool {
        matches!(self, Self::Clean)
    }

    /// Whether its content still occupies the storage (the quota counts it).
    pub const fn holds_content(self) -> bool {
        matches!(self, Self::Pending | Self::Scanning | Self::Clean)
    }
}

/// Longest file name, in characters.
pub const NAME_MAX_CHARS: usize = 255;

/// The name to keep for a file the browser named `raw`: its last path
/// component, without control characters, trimmed; none if nothing is left.
pub fn clean_name(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = last.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name.chars().take(NAME_MAX_CHARS).collect())
}

/// Why an upload is refused before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadRefusal {
    Empty,
    /// Larger than the configured maximum.
    TooLarge,
    /// The tenant's storage quota would be exceeded.
    QuotaExceeded,
}

impl UploadRefusal {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLarge => "too_large",
            Self::QuotaExceeded => "quota_exceeded",
        }
    }
}

/// Whether a file of `size` bytes may be uploaded, the maximum being
/// `max`, the tenant using `used` bytes of its `quota` (none: unlimited).
pub fn check_upload(size: u64, max: u64, used: u64, quota: Option<u64>) -> Result<(), UploadRefusal> {
    if size == 0 {
        return Err(UploadRefusal::Empty);
    }
    if size > max {
        return Err(UploadRefusal::TooLarge);
    }
    if quota.is_some_and(|quota| used.saturating_add(size) > quota) {
        return Err(UploadRefusal::QuotaExceeded);
    }
    Ok(())
}

/// The images thumbnails are made of, told by their first bytes, never by
/// the name or type the browser gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    WebP,
}

impl ImageKind {
    /// The kind of image `head` (the first bytes of a file) starts, if any.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
            Some(Self::WebP)
        } else {
            None
        }
    }

    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
        }
    }
}

/// Largest image a thumbnail is made of, in bytes: decoding is bounded.
pub const THUMBNAIL_SOURCE_MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Largest width or height of an image a thumbnail is made of.
pub const THUMBNAIL_SOURCE_MAX_PIXELS: u32 = 10_000;
/// Width and height a thumbnail fits in.
pub const THUMBNAIL_SIZE: u32 = 320;

/// The type a clean file is served with: images as what they are (told by
/// their bytes), anything else as plain bytes. Downloads are always
/// attachments, so the browser never renders them on the application's
/// origin.
pub fn served_type(image: Option<ImageKind>) -> &'static str {
    image.map_or("application/octet-stream", ImageKind::media_type)
}

/// What the member acting may do with attachments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acting {
    pub account: u128,
    /// Holds `edit` on the ticket's project: adds attachments.
    pub edits: bool,
    /// Administers the project (or the tenant): removes any.
    pub moderates: bool,
}

/// Whether `acting` may remove the attachment `uploader` added (none: an
/// erased account): its uploader while they edit the project, or a
/// moderator.
pub fn can_delete(uploader: Option<u128>, acting: &Acting) -> bool {
    acting.moderates || (acting.edits && uploader == Some(acting.account))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_and_only_clean_files_download() {
        for status in Status::ALL {
            assert_eq!(Status::parse(status.as_str()), Some(status));
            assert_eq!(status.downloadable(), status == Status::Clean);
        }
        assert!(Status::Scanning.holds_content());
        assert!(!Status::Infected.holds_content());
        assert!(!Status::Rejected.holds_content());
    }

    #[test]
    fn names_keep_their_last_component_without_control_characters() {
        assert_eq!(clean_name("Plan v2.pdf").as_deref(), Some("Plan v2.pdf"));
        assert_eq!(clean_name("C:\\Users\\lb\\scan.png").as_deref(), Some("scan.png"));
        assert_eq!(clean_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(clean_name("  a\u{0}b\n.txt ").as_deref(), Some("ab.txt"));
        assert_eq!(clean_name("dir/"), None);
        assert_eq!(clean_name(".."), None);
        assert_eq!(
            clean_name(&"é".repeat(300)).map(|n| n.chars().count()),
            Some(NAME_MAX_CHARS)
        );
    }

    #[test]
    fn uploads_respect_the_maximum_and_the_quota() {
        let mb = 1024 * 1024;
        assert_eq!(check_upload(3 * mb, 50 * mb, 0, None), Ok(()));
        assert_eq!(check_upload(0, 50 * mb, 0, None), Err(UploadRefusal::Empty));
        assert_eq!(
            check_upload(51 * mb, 50 * mb, 0, None),
            Err(UploadRefusal::TooLarge)
        );
        assert_eq!(
            check_upload(50 * mb, 50 * mb, 0, None),
            Ok(()),
            "the maximum itself fits"
        );
        assert_eq!(
            check_upload(3 * mb, 50 * mb, 98 * mb, Some(100 * mb)),
            Err(UploadRefusal::QuotaExceeded)
        );
        assert_eq!(
            check_upload(2 * mb, 50 * mb, 98 * mb, Some(100 * mb)),
            Ok(()),
            "up to the quota"
        );
        assert_eq!(UploadRefusal::QuotaExceeded.code(), "quota_exceeded");
    }

    #[test]
    fn images_are_told_by_their_bytes() {
        assert_eq!(ImageKind::sniff(b"\x89PNG\r\n\x1a\n...."), Some(ImageKind::Png));
        assert_eq!(ImageKind::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageKind::Jpeg));
        assert_eq!(ImageKind::sniff(b"GIF89a..."), Some(ImageKind::Gif));
        assert_eq!(ImageKind::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageKind::WebP));
        assert_eq!(ImageKind::sniff(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(
            ImageKind::sniff(b"<svg xmlns=..."),
            None,
            "SVG can hold scripts: never an image here"
        );
        assert_eq!(served_type(Some(ImageKind::Png)), "image/png");
        assert_eq!(served_type(None), "application/octet-stream");
    }

    #[test]
    fn uploaders_who_edit_and_moderators_remove_attachments() {
        let acting = |edits, moderates| Acting {
            account: 1,
            edits,
            moderates,
        };
        assert!(can_delete(Some(1), &acting(true, false)));
        assert!(
            !can_delete(Some(1), &acting(false, false)),
            "no longer edits the project"
        );
        assert!(!can_delete(Some(2), &acting(true, false)));
        assert!(can_delete(Some(2), &acting(false, true)));
        assert!(can_delete(None, &acting(false, true)));
    }
}
