//! Published API rates for the models local logs name. A model matches
//! its longest prefix; anything else stays unpriced, never $0.
//!
//! Rates are USD per million tokens. Checked 2026-11-25 against:
//! - Claude: https://platform.claude.com/docs/en/about-claude/pricing
//! - OpenAI: https://developers.openai.com/api/docs/pricing (Standard tier)

pub(crate) const CLAUDE_SOURCE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
pub(crate) const OPENAI_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";
pub(crate) const CHECKED: &str = "2026-11-25";

/// Fresh input, cache read, 5-minute cache write, 1-hour cache write and
/// output, per million tokens. Cache-write rates are None where the
/// provider publishes none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rates {
    pub(crate) input: f64,
    pub(crate) cache_read: f64,
    pub(crate) write_5m: Option<f64>,
    pub(crate) write_1h: Option<f64>,
    pub(crate) output: f64,
}

pub(crate) struct Entry {
    pub(crate) prefix: &'static str,
    pub(crate) rates: Rates,
    /// Prompts (all input tokens) above this size pay `long` instead, or
    /// are unpriced when `long` is None.
    pub(crate) prompt_limit: Option<(u64, Option<Rates>)>,
    pub(crate) checked: &'static str,
    pub(crate) source: &'static str,
}

const fn claude(prefix: &'static str, input: f64, read: f64, output: f64) -> Entry {
    Entry {
        prefix,
        rates: Rates {
            input,
            cache_read: read,
            write_5m: Some(input * 1.25),
            write_1h: Some(input * 2.0),
            output,
        },
        prompt_limit: None,
        checked: CHECKED,
        source: CLAUDE_SOURCE,
    }
}

/// OpenAI's Standard rates cover prompts up to 272K tokens; longer
/// prompts are priced differently, so they stay unpriced here.
const fn openai(prefix: &'static str, input: f64, read: f64, output: f64) -> Entry {
    Entry {
        prefix,
        rates: Rates {
            input,
            cache_read: read,
            write_5m: None,
            write_1h: None,
            output,
        },
        prompt_limit: Some((272_000, None)),
        checked: CHECKED,
        source: OPENAI_SOURCE,
    }
}

pub(crate) const TABLE: [Entry; 18] = [
    claude("claude-fable-5-2", 8.0, 0.20, 40.0),
    claude("claude-fable-5-1", 10.0, 0.25, 50.0),
    claude("claude-fable-5", 10.0, 1.0, 50.0),
    claude("claude-opus-5-5", 4.0, 0.20, 20.0),
    claude("claude-opus-5", 5.0, 0.50, 25.0),
    claude("claude-opus-4-8", 5.0, 0.50, 25.0),
    claude("claude-opus-4-7", 5.0, 0.50, 25.0),
    claude("claude-opus-4-6", 5.0, 0.50, 25.0),
    claude("claude-sonnet-5-5", 2.0, 0.10, 10.0),
    claude("claude-sonnet-5", 2.0, 0.20, 10.0),
    claude("claude-sonnet-4-6", 3.0, 0.30, 15.0),
    Entry {
        prompt_limit: Some((
            100_000,
            Some(Rates {
                input: 0.50,
                cache_read: 0.05,
                write_5m: Some(0.625),
                write_1h: Some(1.0),
                output: 2.50,
            }),
        )),
        ..claude("claude-haiku-5-5", 0.10, 0.01, 0.50)
    },
    claude("claude-haiku-4-5", 1.0, 0.10, 5.0),
    openai("gpt-6-astra", 10.0, 1.0, 50.0),
    openai("gpt-6-luna", 0.10, 0.01, 0.50),
    openai("gpt-6.1-sol", 2.0, 0.10, 10.0),
    openai("gpt-6.2-astra", 8.0, 0.40, 40.0),
    openai("gpt-6.2-sol", 2.0, 0.10, 10.0),
];

pub(crate) fn lookup(model: &str) -> Option<&'static Entry> {
    lookup_in(&TABLE, model)
}

fn lookup_in<'a>(table: &'a [Entry], model: &str) -> Option<&'a Entry> {
    table
        .iter()
        .filter(|e| model.starts_with(e.prefix))
        .max_by_key(|e| e.prefix.len())
}

/// One request's token parts. `write_5m + write_1h` is the cache write.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Usage {
    pub(crate) fresh: u64,
    pub(crate) cache_read: u64,
    pub(crate) write_5m: u64,
    pub(crate) write_1h: u64,
    pub(crate) output: u64,
}

/// Cents per part (fresh input, cache read, cache write, output), or None
/// when the model has no rate, the prompt is outside the rates' range, or
/// a cache write has no published rate.
pub(crate) fn price(model: &str, u: &Usage) -> Option<[f64; 4]> {
    price_with(lookup(model)?, u)
}

fn price_with(entry: &Entry, u: &Usage) -> Option<[f64; 4]> {
    let prompt = u.fresh + u.cache_read + u.write_5m + u.write_1h;
    let rates = match entry.prompt_limit {
        Some((limit, long)) if prompt > limit => long?,
        _ => entry.rates,
    };
    let cents = |tokens: u64, rate: f64| tokens as f64 * rate / 1e6 * 100.0;
    let write = |tokens: u64, rate: Option<f64>| match tokens {
        0 => Some(0.0),
        _ => rate.map(|r| cents(tokens, r)),
    };
    Some([
        cents(u.fresh, rates.input),
        cents(u.cache_read, rates.cache_read),
        write(u.write_5m, rates.write_5m)? + write(u.write_1h, rates.write_1h)?,
        cents(u.output, rates.output),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // A model must price at its own family's rate, never a shorter, older
    // prefix's: Opus 5.5 costs less than Opus 5.
    #[test]
    fn longest_prefix_wins() {
        let table = [
            claude("claude-opus", 1.0, 0.1, 1.0),
            claude("claude-opus-5", 5.0, 0.5, 25.0),
        ];
        assert_eq!(
            lookup_in(&table, "claude-opus-5-5").unwrap().prefix,
            "claude-opus-5"
        );
        assert_eq!(lookup("claude-opus-5-5").unwrap().prefix, "claude-opus-5-5");
        assert!(lookup("claude-unknown-9").is_none());
    }

    // Most cache writes are 1-hour writes, which cost 2x input, not 1.25x.
    #[test]
    fn cache_writes_use_their_own_rate() {
        let u = Usage {
            write_1h: 1_000,
            write_5m: 200,
            ..Usage::default()
        };
        let cents = price("claude-opus-5", &u).unwrap();
        // 1,000 x $10/M + 200 x $6.25/M = $0.01125 = 1.125 cents.
        assert!((cents[2] - 1.125).abs() < 1e-9, "{cents:?}");
    }

    // A long Haiku 5.5 prompt pays the higher tier; a long OpenAI prompt
    // is outside the published Standard rates and stays unpriced.
    #[test]
    fn prompt_limits_change_or_drop_the_rate() {
        let long = Usage {
            fresh: 150_000,
            output: 10,
            ..Usage::default()
        };
        let haiku = price("claude-haiku-5-5", &long).unwrap();
        assert!((haiku[0] - 7.5).abs() < 1e-9, "{haiku:?}");
        assert!(price("gpt-6.1-sol", &long).is_some());
        let longer = Usage {
            fresh: 300_000,
            ..Usage::default()
        };
        assert!(price("gpt-6.1-sol", &longer).is_none());
    }

    // OpenAI publishes no cache-write rate, so a Codex cache write can't
    // be priced. Guessing a rate would make the total look exact.
    #[test]
    fn unpublished_cache_write_is_unpriced() {
        let u = Usage {
            fresh: 10,
            write_5m: 5,
            ..Usage::default()
        };
        assert!(price("gpt-6.1-sol", &u).is_none());
        assert!(price("codex-auto-review", &Usage::default()).is_none());
    }
}
