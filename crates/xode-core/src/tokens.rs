use once_cell::sync::Lazy;
use tiktoken_rs::CoreBPE;

static BPE: Lazy<Option<CoreBPE>> = Lazy::new(|| tiktoken_rs::cl100k_base().ok());

/// Fast token estimate. Uses cl100k for short strings, byte heuristic for large blobs.
pub fn count(s: &str) -> u64 {
    if s.is_empty() {
        return 0;
    }
    if s.len() > 200_000 {
        return (s.len() as u64) / 3;
    }
    match &*BPE {
        Some(b) => b.encode_ordinary(s).len() as u64,
        None => (s.len() as u64) / 3 + 1,
    }
}

/// Very cheap estimate (no BPE) for hot paths.
pub fn quick(s: &str) -> u64 {
    (s.len() as u64 + 2) / 3
}
