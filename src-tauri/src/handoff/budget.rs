//! How much of the target model's context window a handoff may take (docs/handoff.md#budget).
//!
//! All in whole numbers, so the same thread and model always give the same handoff:
//!
//! ```text
//! window     = the catalog's window for the target model, else 128,000 tokens
//! reserve    = max(16,000, window / 4)        instructions, tools and the work to come
//! request    = ceil(request bytes / 3) + 8,192 per attached image
//! allowance  = min(window / 4, window - reserve - request - native) tokens, × 3 bytes,
//!              at most 262,144 bytes; under 4,096 bytes nothing is sent
//! split      = summary 30 %, references 10 %, recent turns the remaining 60 %
//! ```
//!
//! The reserve, the assumed window and the image estimate are ViewCode's. ViewCode charges one
//! token per byte and stops at 16,000 tokens; here text is charged at three bytes per token and
//! the allowance grows with the window, so a large model receives more of the thread in full.

/// Assumed when neither the catalog nor an earlier turn reported the model's window.
pub(crate) const DEFAULT_WINDOW: u64 = 128_000;
/// A conservative estimate for prose and code alike; real tokenizers average more.
pub(crate) const BYTES_PER_TOKEN: u64 = 3;
/// Kept free for the agent's instructions, its tools and the turns that follow.
pub(crate) const RESERVE_MIN_TOKENS: u64 = 16_000;
/// Reserved per image attached to the current request, whatever its file size.
pub(crate) const IMAGE_TOKENS: u64 = 8_192;
/// The handoff's share of the window.
pub(crate) const WINDOW_SHARE: u64 = 4;
/// No handoff is longer than this, whatever the window.
pub(crate) const MAX_BYTES: usize = 256 * 1024;
/// Below this not even the opening and the pointers fit.
pub(crate) const MIN_BYTES: usize = 4 * 1024;
/// Always kept for the pointers to what is not shown.
pub(crate) const REFERENCE_MIN: usize = 400;
pub(crate) const SUMMARY_PERCENT: usize = 30;
pub(crate) const REFERENCE_PERCENT: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    pub window: u64,
    /// False when `DEFAULT_WINDOW` stood in.
    pub window_known: bool,
    pub reserve_tokens: u64,
    pub request_tokens: u64,
    pub allowance_bytes: usize,
    /// The most the structured summary may take.
    pub summary_bytes: usize,
    /// What recent turns get before the summary; they also take what the summary leaves.
    pub verbatim_bytes: usize,
    pub reference_bytes: usize,
}

impl Budget {
    /// A single message longer than this is shown as an excerpt of half this size.
    pub(crate) fn long_message(&self) -> usize {
        (self.allowance_bytes / 8).clamp(2_000, 24_000)
    }
}

/// `native_tokens` is what the target's own resumed session already holds (0 for a fresh one).
pub(crate) fn budget(window: Option<u64>, native_tokens: u64, request_bytes: usize, images: usize) -> Budget {
    let known = window.filter(|window| *window > 0);
    let window = known.unwrap_or(DEFAULT_WINDOW);
    let reserve_tokens = RESERVE_MIN_TOKENS.max(window / 4);
    let request_tokens = (request_bytes as u64).div_ceil(BYTES_PER_TOKEN) + images as u64 * IMAGE_TOKENS;
    let free = window.saturating_sub(reserve_tokens).saturating_sub(request_tokens).saturating_sub(native_tokens);
    let allowance_bytes = usize::try_from((window / WINDOW_SHARE).min(free).saturating_mul(BYTES_PER_TOKEN)).unwrap_or(MAX_BYTES).min(MAX_BYTES);
    let summary_bytes = allowance_bytes * SUMMARY_PERCENT / 100;
    let reference_bytes = allowance_bytes * REFERENCE_PERCENT / 100;
    Budget {
        window,
        window_known: known.is_some(),
        reserve_tokens,
        request_tokens,
        allowance_bytes,
        summary_bytes,
        verbatim_bytes: allowance_bytes - summary_bytes - reference_bytes,
        reference_bytes,
    }
}
