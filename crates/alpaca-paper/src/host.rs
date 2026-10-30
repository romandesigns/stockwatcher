//! The paper-host lock. Every URL this crate sends anything to (REST and the trade stream) goes
//! through [`ensure_paper`] immediately before the request leaves, and the base is a constant:
//! there is no setting, environment variable or constructor argument that can point this crate
//! at another host. `api.alpaca.markets` (live money) is refused like every other host.
//!
//! Going live later must be an explicit code change reviewed as such (a new constant, a new
//! check and new tests), never a configuration flip.

/// The only trading host this crate talks to.
pub const PAPER_HOST: &str = "paper-api.alpaca.markets";
/// REST base of the paper Trading API.
pub const PAPER_BASE: &str = "https://paper-api.alpaca.markets";
/// The paper account's trade updates stream.
pub const PAPER_STREAM: &str = "wss://paper-api.alpaca.markets/stream";

/// A URL that is not the paper host. Carries the reason, never the URL itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRefused(pub &'static str);

impl std::fmt::Display for HostRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused: {} (only {PAPER_HOST} is allowed)", self.0)
    }
}

impl std::error::Error for HostRefused {}

/// Ok only for `https://paper-api.alpaca.markets/...` or `wss://paper-api.alpaca.markets/...`:
/// exact host (case-insensitive), no port, no user info.
pub fn ensure_paper(url: &str) -> Result<(), HostRefused> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or(HostRefused("not an absolute URL"))?;
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("wss") {
        return Err(HostRefused("only https and wss are allowed"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(HostRefused("user info in the URL"));
    }
    if authority.contains(':') {
        return Err(HostRefused("explicit port"));
    }
    if !authority.eq_ignore_ascii_case(PAPER_HOST) {
        return Err(HostRefused("not the Alpaca paper trading host"));
    }
    Ok(())
}

/// `PAPER_BASE` + a path starting with `/`.
pub fn paper_url(path_and_query: &str) -> String {
    debug_assert!(path_and_query.starts_with('/'));
    format!("{PAPER_BASE}{path_and_query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_paper_host_passes() {
        assert!(ensure_paper(PAPER_BASE).is_ok());
        assert!(ensure_paper(&paper_url("/v2/orders?status=open")).is_ok());
        assert!(ensure_paper(PAPER_STREAM).is_ok());
        assert!(ensure_paper("HTTPS://PAPER-API.ALPACA.MARKETS/v2/account").is_ok());
    }

    #[test]
    fn the_live_money_host_is_refused() {
        for url in [
            "https://api.alpaca.markets/v2/orders",
            "wss://api.alpaca.markets/stream",
            "https://api.alpaca.markets",
        ] {
            assert_eq!(
                ensure_paper(url),
                Err(HostRefused("not the Alpaca paper trading host")),
                "{url}"
            );
        }
    }

    #[test]
    fn look_alikes_and_tricks_are_refused() {
        for url in [
            "http://paper-api.alpaca.markets/v2/orders",
            "https://paper-api.alpaca.markets.evil.example/v2/orders",
            "https://evil.example/paper-api.alpaca.markets",
            "https://paper-api.alpaca.markets@api.alpaca.markets/v2/orders",
            "https://paper-api.alpaca.markets:8443/v2/orders",
            "https://data.alpaca.markets/v2/stocks",
            "paper-api.alpaca.markets/v2/orders",
            "",
        ] {
            assert!(ensure_paper(url).is_err(), "{url}");
        }
    }

    #[test]
    fn the_constants_themselves_are_paper() {
        assert!(ensure_paper(PAPER_BASE).is_ok() && PAPER_BASE.contains("paper-api."));
        assert!(ensure_paper(PAPER_STREAM).is_ok() && PAPER_STREAM.contains("paper-api."));
    }
}
