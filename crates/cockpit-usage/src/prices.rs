//! What a reply costs at the provider's published API prices (the plan itself is flat, so
//! this is what the same tokens would bill on API credits). Prices are integers, in
//! nano-dollars per token, which is dollars per million tokens times 1000: every published
//! price has at most three decimals, so a reply's cost and every total of them is exact.

use cockpit_provider_api::ReplyTokens;

/// Nano-dollars per token: input, output, cache read, cache write for five minutes, cache
/// write for one hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Prices([u64; 5]);

/// The Claude API reference's standard rates, as the legacy cockpit kept them (cached
/// 2026-10-06). A new model is a new row; a reply of a model without one is archived without
/// a cost, never with a guessed one.
const CLAUDE_PRICES: &[(&str, Prices)] = &[
    (
        "claude-fable-5-1",
        Prices([10_000, 50_000, 250, 12_500, 20_000]),
    ),
    (
        "claude-mythos-5-1",
        Prices([10_000, 50_000, 250, 12_500, 20_000]),
    ),
    (
        "claude-fable-5",
        Prices([10_000, 50_000, 1_000, 12_500, 20_000]),
    ),
    (
        "claude-mythos-5",
        Prices([10_000, 50_000, 1_000, 12_500, 20_000]),
    ),
    (
        "claude-opus-5-5",
        Prices([4_000, 20_000, 200, 5_000, 8_000]),
    ),
    ("claude-opus-5", Prices([5_000, 25_000, 500, 6_250, 10_000])),
    (
        "claude-opus-4-8",
        Prices([5_000, 25_000, 500, 6_250, 10_000]),
    ),
    (
        "claude-opus-4-7",
        Prices([5_000, 25_000, 500, 6_250, 10_000]),
    ),
    (
        "claude-opus-4-6",
        Prices([5_000, 25_000, 500, 6_250, 10_000]),
    ),
    (
        "claude-opus-4-5",
        Prices([5_000, 25_000, 500, 6_250, 10_000]),
    ),
    (
        "claude-sonnet-5-5",
        Prices([2_000, 10_000, 200, 2_500, 4_000]),
    ),
    (
        "claude-sonnet-5",
        Prices([2_000, 10_000, 200, 2_500, 4_000]),
    ),
    (
        "claude-sonnet-4-6",
        Prices([3_000, 15_000, 300, 3_750, 6_000]),
    ),
    ("claude-haiku-5-5", Prices([100, 500, 10, 125, 200])),
    (
        "claude-haiku-4-5",
        Prices([1_000, 5_000, 100, 1_250, 2_000]),
    ),
];

/// Haiku 5.5 bills a prompt past 100 000 tokens at these rates instead.
const HAIKU_5_5_LONG_PROMPT_TOKENS: u64 = 100_000;
const HAIKU_5_5_LONG_PROMPT: Prices = Prices([500, 2_500, 50, 625, 1_000]);
/// Fast mode bills every token at twice the standard rate.
const FAST_MODE_MULTIPLIER: u64 = 2;

/// The most tokens one count of a reply may hold. Real replies are far below it; a count past
/// it is not a reply this archive believes, so it is refused before any arithmetic.
pub(crate) const MAX_TOKENS_PER_COUNT: u64 = 1_000_000_000_000;

/// What `tokens` of `model` cost, in nano-dollars; `None` for a model without a known price.
/// `tokens` hold at most [`MAX_TOKENS_PER_COUNT`] each.
pub(crate) fn cost_nanos(model: &str, tokens: ReplyTokens) -> Option<u64> {
    let mut prices = prices_of(model)?;
    let prompt = tokens.input + tokens.cache_read + tokens.cache_write_5m + tokens.cache_write_1h;
    if model.starts_with("claude-haiku-5-5") && prompt > HAIKU_5_5_LONG_PROMPT_TOKENS {
        prices = HAIKU_5_5_LONG_PROMPT;
    }
    let [input, output, cache_read, write_5m, write_1h] = prices.0;
    let nanos = tokens.input * input
        + tokens.output * output
        + tokens.cache_read * cache_read
        + tokens.cache_write_5m * write_5m
        + tokens.cache_write_1h * write_1h;
    Some(if tokens.is_fast {
        nanos * FAST_MODE_MULTIPLIER
    } else {
        nanos
    })
}

/// The row of a model id, a dated one (`-20251101`, `-2025-11-01`) included: the longest
/// known id it starts with wins, so `claude-opus-5-5` never takes `claude-opus-5`'s row.
fn prices_of(model: &str) -> Option<Prices> {
    CLAUDE_PRICES
        .iter()
        .filter(|(known, _)| {
            model == *known
                || model.strip_prefix(known).is_some_and(|rest| {
                    rest.strip_prefix('-').is_some_and(|date| {
                        matches!(date.len(), 8 | 10)
                            && date
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || byte == b'-')
                    })
                })
        })
        .max_by_key(|(known, _)| known.len())
        .map(|(_, prices)| *prices)
}
