//! Paper credentials, from the server environment only. They never reach a client, a log line, an
//! error message or a `Debug` print: [`Credentials`] redacts itself, and only the HTTP layer and
//! the stream's auth message (both in this crate) read the values.

/// Where the keys came from (reported in the settings status by name, never the values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredSource {
    /// `ALPACA_PAPER_KEY` / `ALPACA_PAPER_SECRET`: dedicated paper keys (preferred).
    Paper,
    /// `ALPACA_API_KEY` / `ALPACA_API_SECRET` (or `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY`), the
    /// market data keys. Usable only when they are paper keys: the paper host refuses live keys
    /// (401), and that refusal is reported as "not configured".
    Shared,
}

impl CredSource {
    pub fn env_names(self) -> &'static str {
        match self {
            CredSource::Paper => "ALPACA_PAPER_KEY / ALPACA_PAPER_SECRET",
            CredSource::Shared => "ALPACA_API_KEY / ALPACA_API_SECRET",
        }
    }
}

#[derive(Clone)]
pub struct Credentials {
    key: String,
    secret: String,
    pub source: CredSource,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("key", &"<redacted>")
            .field("secret", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

/// Why no usable credentials were found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredsMissing {
    /// Nothing set at all.
    None,
    /// Only the shared market data keys are set and they look like live-account keys (Alpaca's
    /// live key ids start with `AK`, paper ones with `PK`). Not even tried against paper.
    SharedLooksLive,
}

impl Credentials {
    pub fn new(key: impl Into<String>, secret: impl Into<String>, source: CredSource) -> Self {
        Credentials {
            key: key.into(),
            secret: secret.into(),
            source,
        }
    }

    /// Dedicated paper names first, then the shared names when they do not look like live keys.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, CredsMissing> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        if let (Some(k), Some(s)) = (var("ALPACA_PAPER_KEY"), var("ALPACA_PAPER_SECRET")) {
            return Ok(Credentials::new(k, s, CredSource::Paper));
        }
        let shared = match (var("ALPACA_API_KEY"), var("ALPACA_API_SECRET")) {
            (Some(k), Some(s)) => Some((k, s)),
            _ => match (var("APCA_API_KEY_ID"), var("APCA_API_SECRET_KEY")) {
                (Some(k), Some(s)) => Some((k, s)),
                _ => None,
            },
        };
        match shared {
            None => Err(CredsMissing::None),
            Some((k, _)) if k.starts_with("AK") => Err(CredsMissing::SharedLooksLive),
            Some((k, s)) => Ok(Credentials::new(k, s, CredSource::Shared)),
        }
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn secret(&self) -> &str {
        &self.secret
    }

    /// Replaces every occurrence of either value in `text` (an Alpaca error body quoted back).
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for s in [&self.key, &self.secret] {
            if s.len() >= 4 {
                out = out.replace(s.as_str(), "<redacted>");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn dedicated_paper_names_win() {
        let c = Credentials::from_lookup(env(&[
            ("ALPACA_PAPER_KEY", "PKPAPER"),
            ("ALPACA_PAPER_SECRET", "s1"),
            ("ALPACA_API_KEY", "PKSHARED"),
            ("ALPACA_API_SECRET", "s2"),
        ]))
        .unwrap();
        assert_eq!(c.source, CredSource::Paper);
        assert_eq!(c.key(), "PKPAPER");
    }

    #[test]
    fn shared_names_are_a_fallback_unless_they_look_live() {
        let c = Credentials::from_lookup(env(&[
            ("ALPACA_API_KEY", "PKSHARED"),
            ("ALPACA_API_SECRET", "s2"),
        ]))
        .unwrap();
        assert_eq!(c.source, CredSource::Shared);
        assert_eq!(
            Credentials::from_lookup(env(&[
                ("ALPACA_API_KEY", "AKLIVE"),
                ("ALPACA_API_SECRET", "s2")
            ]))
            .unwrap_err(),
            CredsMissing::SharedLooksLive
        );
        assert_eq!(
            Credentials::from_lookup(env(&[("ALPACA_PAPER_KEY", "PK")])).unwrap_err(),
            CredsMissing::None
        );
    }

    #[test]
    fn debug_and_scrub_never_show_values() {
        let c = Credentials::new("PKSECRETKEYID", "verysecretvalue", CredSource::Paper);
        let d = format!("{c:?}");
        assert!(
            !d.contains("PKSECRETKEYID") && !d.contains("verysecretvalue"),
            "{d}"
        );
        assert_eq!(c.scrub("bad key PKSECRETKEYID"), "bad key <redacted>");
    }
}
