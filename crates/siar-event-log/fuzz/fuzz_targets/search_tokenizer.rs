//! §85/§59: indexed text is hostile-capable input; tokenizing must stay
//! bounded for any string.
//!
//!     cargo +nightly fuzz run search_tokenizer
#![no_main]
use libfuzzer_sys::fuzz_target;
use siar_event_log::search::{tokenize, SearchLimits};

fuzz_target!(|text: &str| {
    let limits = SearchLimits::default();
    let terms = tokenize(text, &limits);
    assert!(terms.len() <= limits.max_terms_per_doc);
    assert!(terms.iter().all(|t| t.chars().count() <= limits.max_term_len));
});
