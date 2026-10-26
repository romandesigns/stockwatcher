// Archive adapter from research/pullback-context replay.rs (2267a1c).
use chrono::NaiveDate;
use market_archive::Archive;
use market_replay::source::{EventSource, EventStream};
pub struct OneSymbol {
    pub archive: Archive,
    pub day: NaiveDate,
    pub symbol: String,
    pub live: bool,
}

impl EventSource for OneSymbol {
    fn open(&self) -> Result<Vec<EventStream>, String> {
        let r = if self.live {
            self.archive.live_reader(self.day, &self.symbol, None)
        } else {
            self.archive.reader(self.day, &self.symbol)
        }
        .map_err(|e| e.to_string())?;
        Ok(vec![Box::new(
            r.map(|x| x.map(|v| v.event).map_err(|e| e.to_string())),
        )])
    }
    fn day(&self) -> NaiveDate {
        self.day
    }
    fn symbols(&self) -> Vec<String> {
        vec![self.symbol.clone()]
    }
    fn fixture(&self) -> bool {
        false
    }
    fn label(&self) -> String {
        format!("research {} {}", self.day, self.symbol)
    }
}
