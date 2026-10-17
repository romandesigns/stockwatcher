//! Where replay reads events from: an archive day, or in-memory streams (tests).

use chrono::NaiveDate;
use market_archive::Archive;
use market_events::session::sessions_for;
use market_events::{MarketEvent, UnixNanos};

/// One symbol's events in arrival order.
pub type EventStream = Box<dyn Iterator<Item = Result<MarketEvent, String>> + Send>;

/// A re-openable set of per-symbol streams for one trading day. `open` is called again on every
/// seek, so it must return the same data each time.
pub trait EventSource: Send + Sync + 'static {
    fn open(&self) -> Result<Vec<EventStream>, String>;
    fn day(&self) -> NaiveDate;
    fn symbols(&self) -> Vec<String>;
    /// True when every stream is fixture data (never market evidence).
    fn fixture(&self) -> bool;
    fn label(&self) -> String;
}

/// `[first session start, last session end)` of a trading day (overnight 20:00 the evening before
/// through the end of after-hours), or `None` for a non-trading day.
pub fn day_bounds(day: NaiveDate) -> Option<(UnixNanos, UnixNanos)> {
    let s = sessions_for(day);
    Some((s.first()?.1, s.last()?.2))
}

/// Every finalized symbol file of one archive day.
pub struct ArchiveDaySource {
    archive: Archive,
    day: NaiveDate,
    symbols: Vec<String>,
    fixture: bool,
}

impl ArchiveDaySource {
    pub fn new(archive: Archive, day: NaiveDate) -> Result<Self, String> {
        let m = archive
            .manifest(day)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no archive manifest for {day}"))?;
        if day_bounds(day).is_none() {
            return Err(format!("{day} is not a trading day"));
        }
        Ok(ArchiveDaySource {
            archive,
            day,
            symbols: m.symbols.keys().cloned().collect(),
            fixture: m.fixture,
        })
    }

    pub fn archive(&self) -> &Archive {
        &self.archive
    }
}

impl EventSource for ArchiveDaySource {
    fn open(&self) -> Result<Vec<EventStream>, String> {
        self.symbols
            .iter()
            .map(|s| {
                let r = self
                    .archive
                    .reader(self.day, s)
                    .map_err(|e| e.to_string())?;
                Ok(
                    Box::new(r.map(|x| x.map(|v| v.event).map_err(|e| e.to_string())))
                        as EventStream,
                )
            })
            .collect()
    }

    fn day(&self) -> NaiveDate {
        self.day
    }

    fn symbols(&self) -> Vec<String> {
        self.symbols.clone()
    }

    fn fixture(&self) -> bool {
        self.fixture
    }

    fn label(&self) -> String {
        format!(
            "archive {}{}",
            self.day,
            if self.fixture { " (fixture)" } else { "" }
        )
    }
}

/// In-memory streams (tests, parity checks).
pub struct VecSource {
    pub day: NaiveDate,
    pub streams: Vec<(String, Vec<MarketEvent>)>,
    pub fixture: bool,
}

impl EventSource for VecSource {
    fn open(&self) -> Result<Vec<EventStream>, String> {
        Ok(self
            .streams
            .iter()
            .map(|(_, v)| Box::new(v.clone().into_iter().map(Ok)) as EventStream)
            .collect())
    }

    fn day(&self) -> NaiveDate {
        self.day
    }

    fn symbols(&self) -> Vec<String> {
        self.streams.iter().map(|(s, _)| s.clone()).collect()
    }

    fn fixture(&self) -> bool {
        self.fixture
    }

    fn label(&self) -> String {
        format!("memory {}", self.day)
    }
}
