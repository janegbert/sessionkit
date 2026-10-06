// What tokens cost: the published per-token prices of the Claude models, and what cache writes,
// cache reads and fast mode do to them. The status line, the cold-cache warning and `usage` all
// price tokens here, so they give the same number for the same call.

/// How long Claude Code keeps a cache write; a longer lifetime costs more to write.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ttl {
    FiveMinutes,
    OneHour,
}

impl Ttl {
    pub fn ms(self) -> f64 {
        match self {
            Ttl::FiveMinutes => 5.0 * 60.0 * 1000.0,
            Ttl::OneHour => 60.0 * 60.0 * 1000.0,
        }
    }

    /// Cache writes cost 1.25x input for 5 minutes and 2x for 1 hour.
    fn write_factor(self) -> f64 {
        match self {
            Ttl::FiveMinutes => 1.25,
            Ttl::OneHour => 2.0,
        }
    }
}

/// USD per million tokens.
#[derive(Clone, Copy)]
struct Price {
    input: f64,
    output: f64,
    read: Option<f64>,
}

/// From the Claude API model table (cached 2026-09-25). Cache reads cost 0.1x input unless a
/// model sets its own price.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", Price { input: 10.0, output: 50.0, read: Some(0.25) }),
    ("claude-mythos-5-1", Price { input: 10.0, output: 50.0, read: Some(0.25) }),
    ("claude-fable-5", Price { input: 10.0, output: 50.0, read: None }),
    ("claude-mythos-5", Price { input: 10.0, output: 50.0, read: None }),
    ("claude-opus-5-5", Price { input: 4.0, output: 20.0, read: Some(0.2) }),
    ("claude-opus-5", Price { input: 5.0, output: 25.0, read: None }),
    ("claude-opus-4-8", Price { input: 5.0, output: 25.0, read: None }),
    ("claude-opus-4-7", Price { input: 5.0, output: 25.0, read: None }),
    ("claude-opus-4-6", Price { input: 5.0, output: 25.0, read: None }),
    ("claude-sonnet-5-5", Price { input: 2.0, output: 10.0, read: None }),
    ("claude-sonnet-5", Price { input: 2.0, output: 10.0, read: None }),
    ("claude-sonnet-4-6", Price { input: 3.0, output: 15.0, read: None }),
    ("claude-haiku-4-5", Price { input: 1.0, output: 5.0, read: None }),
];

/// A model name as in the table, or with a date suffix such as `-20251001`.
fn price_of(model: &str) -> Option<Price> {
    let find = |name: &str| PRICES.iter().find(|(key, _)| *key == name).map(|(_, price)| *price);
    find(model).or_else(|| {
        let bytes = model.as_bytes();
        let dated = bytes.len() > 9 && bytes[bytes.len() - 9] == b'-' && bytes[bytes.len() - 8..].iter().all(u8::is_ascii_digit);
        if dated { find(&model[..model.len() - 9]) } else { None }
    })
}

/// What tokens cost in USD for one model, in standard or fast mode (fast costs twice as much).
#[derive(Clone, Copy)]
pub struct Rate {
    price: Price,
    factor: f64,
}

impl Rate {
    /// None when the model has no known price.
    pub fn of(model: Option<&str>, fast: bool) -> Option<Rate> {
        Some(Rate { price: price_of(model?)?, factor: if fast { 2.0 } else { 1.0 } })
    }

    fn cost(&self, tokens: f64, per_million: f64) -> f64 {
        (tokens * per_million * self.factor) / 1e6
    }

    pub fn input(&self, tokens: f64) -> f64 {
        self.cost(tokens, self.price.input)
    }

    pub fn read(&self, tokens: f64) -> f64 {
        self.cost(tokens, self.price.read.unwrap_or(self.price.input * 0.1))
    }

    pub fn write(&self, tokens: f64, ttl: Ttl) -> f64 {
        self.cost(tokens, self.price.input * ttl.write_factor())
    }

    pub fn output(&self, tokens: f64) -> f64 {
        self.cost(tokens, self.price.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dated_model_name_has_the_price_of_its_model() {
        let rate = Rate::of(Some("claude-haiku-4-5-20251001"), false).unwrap();
        assert_eq!(rate.input(1e6), 1.0);
        assert!(Rate::of(Some("claude-haiku-4-5-2025"), false).is_none());
        assert!(Rate::of(Some("gpt-5"), false).is_none());
        assert!(Rate::of(None, false).is_none());
    }

    #[test]
    fn cache_reads_cost_a_tenth_of_input_unless_the_model_sets_a_price() {
        assert!((Rate::of(Some("claude-sonnet-4-6"), false).unwrap().read(1e6) - 0.3).abs() < 1e-12);
        assert_eq!(Rate::of(Some("claude-opus-5-5"), false).unwrap().read(1e6), 0.2);
    }

    #[test]
    fn a_one_hour_write_costs_more_than_a_five_minute_write() {
        let rate = Rate::of(Some("claude-opus-5"), false).unwrap();
        assert_eq!(rate.write(1e6, Ttl::FiveMinutes), 6.25);
        assert_eq!(rate.write(1e6, Ttl::OneHour), 10.0);
    }

    #[test]
    fn fast_mode_doubles_every_kind_of_token() {
        let (standard, fast) = (Rate::of(Some("claude-opus-5"), false).unwrap(), Rate::of(Some("claude-opus-5"), true).unwrap());
        assert_eq!(fast.input(1e6), 2.0 * standard.input(1e6));
        assert_eq!(fast.read(1e6), 2.0 * standard.read(1e6));
        assert_eq!(fast.write(1e6, Ttl::OneHour), 2.0 * standard.write(1e6, Ttl::OneHour));
        assert_eq!(fast.output(1e6), 2.0 * standard.output(1e6));
    }
}
