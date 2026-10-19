//! Where replay reads events from: an archive day, or in-memory streams (tests).

use chrono::NaiveDate;
use market_archive::{Archive, DayData};
use market_events::session::sessions_for;
use market_events::{MarketEvent, UnixNanos};
use std::sync::Arc;

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
    /// The same origin's source for another day (a day switch, [`crate::Command::Day`]). It must
    /// validate eagerly (manifest present, trading day), because the driver swaps only after this
    /// succeeds and a refusal must leave the running day untouched. Default: a source with one day.
    fn for_day(&self, day: NaiveDate) -> Result<Arc<dyn EventSource>, String> {
        Err(format!(
            "{} cannot switch days (asked for {day})",
            self.label()
        ))
    }
    /// What a consumer needs to know about this source, as one value.
    fn info(&self) -> SourceInfo {
        SourceInfo {
            day: self.day(),
            label: self.label(),
            symbols: self.symbols(),
            fixture: self.fixture(),
        }
    }
}

/// A source's description, carried by [`crate::FeedItem::Source`] on a day switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub day: NaiveDate,
    /// Human label, e.g. `"archive 2026-10-15 (fixture)"`.
    pub label: String,
    pub symbols: Vec<String>,
    /// Every stream is fixture data (never market evidence).
    pub fixture: bool,
}

/// `[first session start, last session end)` of a trading day (overnight 20:00 the evening before
/// through the end of after-hours), or `None` for a non-trading day.
pub fn day_bounds(day: NaiveDate) -> Option<(UnixNanos, UnixNanos)> {
    let s = sessions_for(day);
    Some((s.first()?.1, s.last()?.2))
}

/// Every symbol file of one archive day: the import's (final) when it has completed the day,
/// else the live market's sealed recording of it (provisional, `market_archive::live`), which the
/// nightly import later replaces. A recording still being appended to is refused: replay opens a
/// day's files on every seek, and a day that grows under it would not replay the same twice.
pub struct ArchiveDaySource {
    archive: Archive,
    day: NaiveDate,
    symbols: Vec<String>,
    fixture: bool,
    provisional: bool,
}

impl ArchiveDaySource {
    pub fn new(archive: Archive, day: NaiveDate) -> Result<Self, String> {
        let data = archive.day_data(day);
        if data == Some(DayData::Provisional { sealed: false }) {
            return Err(format!(
                "{day} is still being recorded live; it can be replayed once its session ends"
            ));
        }
        let provisional = data == Some(DayData::Provisional { sealed: true });
        let m = if provisional {
            archive.live_manifest(day)
        } else {
            archive.manifest(day)
        }
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
            provisional,
        })
    }

    /// True when this is the live recording, not the import.
    pub fn provisional(&self) -> bool {
        self.provisional
    }

    pub fn archive(&self) -> &Archive {
        &self.archive
    }
}

impl EventSource for ArchiveDaySource {
    fn open(&self) -> Result<Vec<EventStream>, String> {
        // The import may have replaced the recording since this source was made: a seek then
        // opens the import (same day, complete and corrected), never the deleted recording.
        if self.provisional && self.archive.is_final(self.day) {
            return ArchiveDaySource::new(self.archive.clone(), self.day)?.open();
        }
        self.symbols
            .iter()
            .map(|s| {
                let r = if self.provisional {
                    self.archive.live_reader(self.day, s, None)
                } else {
                    self.archive.reader(self.day, s)
                }
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

    /// Another day of the same archive. The manifest is read here, so a missing day or a
    /// non-trading day fails before the driver touches its running day.
    fn for_day(&self, day: NaiveDate) -> Result<Arc<dyn EventSource>, String> {
        Ok(Arc::new(ArchiveDaySource::new(self.archive.clone(), day)?))
    }

    fn label(&self) -> String {
        format!(
            "archive {}{}{}",
            self.day,
            if self.fixture { " (fixture)" } else { "" },
            if self.provisional {
                " (provisional: recorded live)"
            } else {
                ""
            }
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
