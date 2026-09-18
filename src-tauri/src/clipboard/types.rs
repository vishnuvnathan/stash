/// What one poll tick found. `Unchanged` is the overwhelmingly common case and
/// must cost nothing beyond reading the platform change counter.
pub enum Snapshot {
    Unchanged,
    /// Clipboard changed, but the owner marked it as secret (password managers).
    /// The content is never read into memory on this path.
    Concealed,
    /// Clipboard changed but holds nothing we store (files, rich objects).
    Unsupported,
    Text(String),
    /// Always PNG-encoded, whatever the platform handed us.
    Image(Vec<u8>),
}

/// Per-platform watcher state. Holds the last change counter so an idle tick is
/// one syscall and an integer compare.
#[derive(Default)]
pub struct PollState {
    pub last_change: u64,
    pub primed: bool,
    /// Only used where no change counter exists (Linux), which is why it is
    /// allowed to sit unread on the other platforms.
    #[allow(dead_code)]
    pub last_text_hash: String,
}

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
