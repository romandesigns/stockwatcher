//! Phone notifications through Expo's push service (https://docs.expo.dev/push-notifications/sending-notifications/).
//!
//! Why a hosted service at all (CLAUDE.md "Open source first"; recorded in the commit too): a
//! notification to a phone that is asleep has to go through the platform's own channel (FCM on
//! Android, APNs on iOS); there is no self-hostable path to a locked phone. Expo's push service
//! is free at this volume (a budget of at most 50 a day), already the app's notification layer
//! (expo-notifications was in the binary from the first build), and needs no Firebase or Apple
//! keys on this server. It is a runtime dependency, so it is kept replaceable and optional:
//! if Expo is down or refuses, the alert still reaches every open app over the socket and
//! Activity records the failure (`phone: failed`, reason `push_failed`); nothing else breaks.
//!
//! - **Devices** register their Expo push token over the socket (`push.register`); tokens are
//!   kept in `DATA_DIR/alerts/push-devices.json`, never synced to other clients. At most
//!   [`MAX_DEVICES`]; the oldest goes first. Each device also carries its Runners opt-ins
//!   ([`RunnersOptIn`], `super::runners`): new flags, bursts, running, run and fast-move entries
//!   on by default, breakouts off.
//! - **Batching:** jobs queued within [`BATCH_WINDOW`] go out in one request of up to
//!   [`MAX_BATCH`] messages (Expo's limit is 100 per request). A Runners push never waits for
//!   the window: it goes at once with whatever is already queued, and one that arrives while a
//!   price alert's window is open ends that window.
//! - **Retries:** a network error, a 429 or a 5xx is retried after 2, 4 and 8 s; then the
//!   event is marked failed.
//! - **Tickets and receipts:** a ticket error `DeviceNotRegistered` removes that token at once;
//!   accepted tickets are checked for their receipts after [`RECEIPT_DELAY`] (Expo keeps them
//!   24 h), and a receipt with `DeviceNotRegistered` removes the token too.
//! - **Credentials:** `EXPO_ACCESS_TOKEN` (optional) is sent as a bearer token when the project
//!   has "enhanced push security" on; never logged. `EXPO_PUSH_BASE` overrides the endpoint
//!   (tests and local verification point it at a stub).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub const MAX_DEVICES: usize = 10;
pub const MAX_BATCH: usize = 100;
pub const BATCH_WINDOW: Duration = Duration::from_millis(300);
pub const RECEIPT_DELAY: Duration = Duration::from_secs(15 * 60);
const RECEIPT_CHECK: Duration = Duration::from_secs(5 * 60);
const RETRY_WAITS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];
pub const DEFAULT_BASE: &str = "https://exp.host/--/api/v2";
const FILE: &str = "push-devices.json";

/// An Expo push token: `ExponentPushToken[…]` (or the newer `ExpoPushToken[…]`).
pub fn valid_token(t: &str) -> bool {
    (t.starts_with("ExponentPushToken[") || t.starts_with("ExpoPushToken["))
        && t.ends_with(']')
        && t.len() <= 256
        && t.bytes().all(|b| b.is_ascii_graphic())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub token: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub name: String,
    pub registered_at: i64,
    /// Runners pushes for new flags (`super::runners`); on unless the phone said otherwise.
    #[serde(default = "yes")]
    pub runners_new: bool,
    /// Runners pushes for breakouts; off unless the phone turned them on.
    #[serde(default)]
    pub runners_breakout: bool,
    /// Buying-burst pushes (`crate::rocket::burst`); on unless the phone said otherwise.
    #[serde(default = "yes")]
    pub runners_burst: bool,
    /// Running-entry pushes (`crate::rocket::running`): on for a new registration (owner,
    /// 2026-10-22: "I don't wanna lose opportunities because I'm away from my phone"); a record
    /// saved before they existed stays off until the phone registers again.
    #[serde(default)]
    pub runners_running: bool,
    /// "Run +20%" pushes (`crate::rocket::run`): on unless the phone turned them off, a record
    /// saved before they existed included (same reason).
    #[serde(default = "yes")]
    pub runners_run: bool,
    /// "Fast move" pushes (`crate::rocket::fast`): on unless the phone turned them off, a record
    /// saved before they existed included (same reason).
    #[serde(default = "yes")]
    pub runners_fast: bool,
    /// Owner, 2026-10-23: every evidence-backed panel event notifies. A record saved before
    /// these existed takes each one's default (serde), exactly as a new registration does.
    /// Volume-shelf entry of a Runners stock: on.
    #[serde(default = "yes")]
    pub runners_shelf: bool,
    /// A follow-up on a stock already in Runners: on.
    #[serde(default = "yes")]
    pub runners_follow_up: bool,
    /// Ross / Signals "Crossing": on.
    #[serde(default = "yes")]
    pub ross_crossing: bool,
    #[serde(default = "yes")]
    pub signals_crossing: bool,
    /// Ross / Signals "At trigger": off.
    #[serde(default)]
    pub ross_trigger: bool,
    #[serde(default)]
    pub signals_trigger: bool,
    /// A Halt watcher gauge appearing: off.
    #[serde(default)]
    pub halt_watch: bool,
    /// "Pullback setting up" and its crossing (owner, 2026-10-25): on, a record saved before it
    /// existed included.
    #[serde(default = "yes")]
    pub runners_pullback: bool,
    /// The phone's app has the drawing alerts' tone channels (`drawingTones` in `push.register`;
    /// apps/mobile src/notifications/setup.ts `drawingTonesSupported`: a native build that bundles
    /// the tones). Only such a phone gets a drawing alert's push on a `drawing-*` channel; every
    /// other one (an older APK, even one running newer JS) gets the plain `alerts` channel, which
    /// it has. Absent (older records and apps) = false.
    #[serde(default)]
    pub drawing_tones: bool,
    /// Exchange halts and resumptions in a covered stock (`super::halts`): both on, a record saved
    /// before they existed included (owner-approved plan "halt sounds", 2026-11-16).
    #[serde(default = "yes")]
    pub halted: bool,
    #[serde(default = "yes")]
    pub resumed: bool,
    /// The phone's own sound for each (a `super::halts::SOUNDS` key; None = the event's default).
    /// Stored now, used on a phone that has the halt sound channels ([`Device::halt_sounds`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halted_sound: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_sound: Option<String>,
    /// The phone's app has the halt sound channels (`haltSounds` in `push.register`): release 2's
    /// native build, which bundles the sounds and creates one Android channel per sound
    /// ([`halt_channel`]). Only such a phone gets a halt push on a `halt-*` channel with its own
    /// sound; every other one (every build in release 1) gets the plain `alerts` channel and the
    /// default sound, which it has. Absent (older records and apps) = false.
    #[serde(default)]
    pub halt_sounds: bool,
    /// The N1 detector's alerts (`crate::n1_live`): a record saved before 2026-11-17 has none, so
    /// it reads false (the device keeps its explicit choice; new devices get the ON default).
    #[serde(default)]
    pub n1_shadow: bool,
}

impl Device {
    /// This phone's switches, in the one shape that decides what a notice answers to.
    pub fn kinds(&self) -> super::DeskKinds {
        super::DeskKinds {
            new: self.runners_new,
            breakout: self.runners_breakout,
            burst: self.runners_burst,
            running: self.runners_running,
            run: self.runners_run,
            fast: self.runners_fast,
            shelf: self.runners_shelf,
            follow_up: self.runners_follow_up,
            ross_crossing: self.ross_crossing,
            signals_crossing: self.signals_crossing,
            ross_trigger: self.ross_trigger,
            signals_trigger: self.signals_trigger,
            halt: self.halt_watch,
            pullback: self.runners_pullback,
            halted: self.halted,
            resumed: self.resumed,
            n1: self.n1_shadow,
        }
    }

    /// This phone's sound key for a halt notice's kind: its own choice, else the notice's default.
    pub fn halt_sound(&self, n: &crate::rocket::Notice) -> &'static str {
        use crate::rocket::{NoticeDetail, NoticeKind};
        let own = match n.kind {
            NoticeKind::Halted => self.halted_sound.as_deref(),
            _ => self.resumed_sound.as_deref(),
        };
        own.and_then(super::halts::sound_key)
            .unwrap_or(match &n.detail {
                Some(NoticeDetail::Trading(t)) => t.sound,
                _ if n.kind == NoticeKind::Halted => super::halts::DEFAULT_HALTED_SOUND,
                _ => super::halts::DEFAULT_RESUMED_SOUND,
            })
    }
}

/// The Android channel of a halt push for a phone with the halt sound channels
/// ([`Device::halt_sounds`]): one per sound, because a channel's sound is fixed on Android, so the
/// sound IS the channel (as the drawing tones' `drawing-*` channels). Release 2's app creates them
/// with these ids; "none" is a silent channel. iOS takes the file name ([`halt_ios_sound`]).
pub fn halt_channel(key: &str) -> &'static str {
    match key {
        "siren" => "halt-siren-v1",
        "klaxon" => "halt-klaxon-v1",
        "triple" => "halt-triple-v1",
        "alarm" => "halt-alarm-v1",
        "chime" => "halt-chime-v1",
        "ding" => "halt-ding-v1",
        "rise" => "halt-rise-v1",
        "blip" => "halt-blip-v1",
        _ => "halt-none-v1",
    }
}

/// The bundled file iOS plays for a halt sound (release 2), None for "none" (silent).
pub fn halt_ios_sound(key: &str) -> Option<&'static str> {
    Some(match key {
        "siren" => "halt_siren.wav",
        "klaxon" => "halt_klaxon.wav",
        "triple" => "halt_triple.wav",
        "alarm" => "halt_alarm.wav",
        "chime" => "halt_chime.wav",
        "ding" => "halt_ding.wav",
        "rise" => "halt_rise.wav",
        "blip" => "halt_blip.wav",
        _ => return None,
    })
}

fn yes() -> bool {
    true
}

/// What `push.register` says about the Runners pushes; `None` = keep what the device had (or the
/// default for a new one), so an older app that does not know about them changes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunnersOptIn {
    pub new: Option<bool>,
    pub breakout: Option<bool>,
    pub burst: Option<bool>,
    pub running: Option<bool>,
    pub run: Option<bool>,
    pub fast: Option<bool>,
    pub shelf: Option<bool>,
    pub follow_up: Option<bool>,
    pub ross_crossing: Option<bool>,
    pub signals_crossing: Option<bool>,
    pub ross_trigger: Option<bool>,
    pub signals_trigger: Option<bool>,
    pub halt: Option<bool>,
    pub pullback: Option<bool>,
    /// The phone has the drawing tone channels (Device::drawing_tones); None = unchanged.
    pub drawing_tones: Option<bool>,
    /// Exchange halts and resumptions (`super::halts`); None = unchanged / the default (on).
    pub halted: Option<bool>,
    pub resumed: Option<bool>,
    /// The phone's sound for each, a `super::halts::SOUNDS` key (an unknown key reads as None).
    pub halted_sound: Option<&'static str>,
    pub resumed_sound: Option<&'static str>,
    /// The phone has the halt sound channels (Device::halt_sounds); None = unchanged.
    pub halt_sounds: Option<bool>,
    /// The N1 shadow alerts (`crate::n1_live`); None = unchanged / the default (off).
    pub n1: Option<bool>,
}

/// The newer switches as `push.register` and `runners.notify` spell them (owner, 2026-10-23),
/// flattened into both messages (`crate::ws`). The phone's names are `runnersShelf`,
/// `runnersFollowUp` and `haltWatch`; the desk's `shelf`, `followUp` and `halt`; the Ross and
/// Signals ones are the same in both. Absent = unchanged (phone) or the default (desk).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoreOptIn {
    #[serde(default, alias = "runnersShelf")]
    pub shelf: Option<bool>,
    #[serde(default, alias = "runnersFollowUp")]
    pub follow_up: Option<bool>,
    #[serde(default)]
    pub ross_crossing: Option<bool>,
    #[serde(default)]
    pub signals_crossing: Option<bool>,
    #[serde(default)]
    pub ross_trigger: Option<bool>,
    #[serde(default)]
    pub signals_trigger: Option<bool>,
    #[serde(default, alias = "haltWatch")]
    pub halt: Option<bool>,
    /// "Pullback setting up" (the phone spells it `runnersPullback`, the desk `pullback`).
    #[serde(default, alias = "runnersPullback")]
    pub pullback: Option<bool>,
    /// A phone's `drawingTones` capability (Device::drawing_tones); ignored for a desk.
    #[serde(default)]
    pub drawing_tones: Option<bool>,
    /// Exchange halts and resumptions (`super::halts`), spelled the same by the phone and the
    /// desk.
    #[serde(default)]
    pub halted: Option<bool>,
    #[serde(default)]
    pub resumed: Option<bool>,
    /// A phone's sound for each (`haltedSound`, `resumedSound`; ignored for a desk, which plays its
    /// own). An unknown key is ignored rather than refusing the whole message.
    #[serde(default, deserialize_with = "sound_opt")]
    pub halted_sound: Option<SoundKey>,
    #[serde(default, deserialize_with = "sound_opt")]
    pub resumed_sound: Option<SoundKey>,
    /// A phone's `haltSounds` capability (Device::halt_sounds); ignored for a desk.
    #[serde(default)]
    pub halt_sounds: Option<bool>,
    /// The N1 shadow alerts (the desk spells it `n1`, the phone `n1Shadow`).
    #[serde(default, alias = "n1Shadow")]
    pub n1: Option<bool>,
}

/// A known halt sound key (`super::halts::SOUNDS`), read off the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundKey(pub &'static str);

/// A sound key from the wire: a known one, else None (never an error).
fn sound_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<SoundKey>, D::Error> {
    let v: Option<String> = Option::deserialize(d)?;
    Ok(v.as_deref().and_then(super::halts::sound_key).map(SoundKey))
}

impl MoreOptIn {
    /// These switches added to the six Runners ones.
    pub fn with(self, o: RunnersOptIn) -> RunnersOptIn {
        RunnersOptIn {
            shelf: self.shelf,
            follow_up: self.follow_up,
            ross_crossing: self.ross_crossing,
            signals_crossing: self.signals_crossing,
            ross_trigger: self.ross_trigger,
            signals_trigger: self.signals_trigger,
            halt: self.halt,
            pullback: self.pullback,
            drawing_tones: self.drawing_tones,
            halted: self.halted,
            resumed: self.resumed,
            halted_sound: self.halted_sound.map(|k| k.0),
            resumed_sound: self.resumed_sound.map(|k| k.0),
            halt_sounds: self.halt_sounds,
            n1: self.n1,
            ..o
        }
    }
}

/// The registered phones, persisted on every change (tiny file, temp + rename).
pub struct Devices {
    dir: Option<PathBuf>,
    list: Vec<Device>,
}

impl Devices {
    pub fn in_memory() -> Self {
        Devices {
            dir: None,
            list: Vec::new(),
        }
    }

    pub fn open(dir: &Path) -> Self {
        let list = std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Device>>(&b).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|d| valid_token(&d.token))
            .collect();
        Devices {
            dir: Some(dir.to_owned()),
            list,
        }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn tokens(&self) -> Vec<String> {
        self.list.iter().map(|d| d.token.clone()).collect()
    }

    /// Every phone's token, split by whether it has the drawing tone channels: (with, without).
    pub fn tokens_by_drawing_tones(&self) -> (Vec<String>, Vec<String>) {
        let (with, without): (Vec<&Device>, Vec<&Device>) =
            self.list.iter().partition(|d| d.drawing_tones);
        let tok = |v: Vec<&Device>| v.into_iter().map(|d| d.token.clone()).collect();
        (tok(with), tok(without))
    }

    /// The phones that want a halt notice, grouped by how their push sounds: (sound key, has the
    /// halt sound channels, tokens). In release 1 no phone has the channels, so each group rings
    /// with the default sound on the `alerts` channel and only its data's `sound` differs.
    pub fn halt_groups(&self, n: &crate::rocket::Notice) -> Vec<(&'static str, bool, Vec<String>)> {
        let mut out: Vec<(&'static str, bool, Vec<String>)> = Vec::new();
        for d in self.list.iter().filter(|d| d.kinds().wants(n)) {
            let key = (d.halt_sound(n), d.halt_sounds);
            match out.iter_mut().find(|(k, c, _)| (*k, *c) == key) {
                Some((_, _, t)) => t.push(d.token.clone()),
                None => out.push((key.0, key.1, vec![d.token.clone()])),
            }
        }
        out
    }

    /// The phones that opted in to this kind of Runners push.
    pub fn runner_tokens(&self, n: &crate::rocket::Notice) -> Vec<String> {
        self.list
            .iter()
            .filter(|d| d.kinds().wants(n))
            .map(|d| d.token.clone())
            .collect()
    }

    pub fn get(&self, token: &str) -> Option<&Device> {
        self.list.iter().find(|d| d.token == token)
    }

    fn save(&self) {
        let Some(dir) = &self.dir else { return };
        let res = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(format!("{FILE}.tmp"));
            std::fs::write(
                &tmp,
                serde_json::to_vec(&self.list).map_err(std::io::Error::other)?,
            )?;
            std::fs::rename(&tmp, dir.join(FILE))
        })();
        if let Err(e) = res {
            tracing::warn!(error = %e, "push devices: save failed");
        }
    }

    /// Adds or refreshes a phone. False when the token is not an Expo push token.
    pub fn register(
        &mut self,
        token: &str,
        platform: &str,
        name: &str,
        now: i64,
        runners: RunnersOptIn,
    ) -> bool {
        if !valid_token(token) {
            return false;
        }
        let had = self.get(token);
        let runners_new = runners.new.or(had.map(|d| d.runners_new)).unwrap_or(true);
        let runners_breakout = runners
            .breakout
            .or(had.map(|d| d.runners_breakout))
            .unwrap_or(false);
        let runners_burst = runners
            .burst
            .or(had.map(|d| d.runners_burst))
            .unwrap_or(true);
        let runners_running = runners
            .running
            .or(had.map(|d| d.runners_running))
            .unwrap_or(true);
        let runners_run = runners.run.or(had.map(|d| d.runners_run)).unwrap_or(true);
        let runners_fast = runners.fast.or(had.map(|d| d.runners_fast)).unwrap_or(true);
        // The newer switches: what the phone says, else what it had, else the default
        // (super::DeskKinds::from_opt_in).
        let def = super::DeskKinds::default();
        let pick = |v: Option<bool>, had_v: fn(&Device) -> bool, d: bool| {
            v.or(had.map(had_v)).unwrap_or(d)
        };
        let runners_shelf = pick(runners.shelf, |d| d.runners_shelf, def.shelf);
        let runners_follow_up = pick(runners.follow_up, |d| d.runners_follow_up, def.follow_up);
        let ross_crossing = pick(
            runners.ross_crossing,
            |d| d.ross_crossing,
            def.ross_crossing,
        );
        let signals_crossing = pick(
            runners.signals_crossing,
            |d| d.signals_crossing,
            def.signals_crossing,
        );
        let ross_trigger = pick(runners.ross_trigger, |d| d.ross_trigger, def.ross_trigger);
        let signals_trigger = pick(
            runners.signals_trigger,
            |d| d.signals_trigger,
            def.signals_trigger,
        );
        let halt_watch = pick(runners.halt, |d| d.halt_watch, def.halt);
        let runners_pullback = pick(runners.pullback, |d| d.runners_pullback, def.pullback);
        let drawing_tones = pick(runners.drawing_tones, |d| d.drawing_tones, false);
        let halted = pick(runners.halted, |d| d.halted, def.halted);
        let resumed = pick(runners.resumed, |d| d.resumed, def.resumed);
        let halt_sounds = pick(runners.halt_sounds, |d| d.halt_sounds, false);
        let n1_shadow = pick(runners.n1, |d| d.n1_shadow, def.n1);
        let sound = |v: Option<&'static str>, had_v: fn(&Device) -> Option<String>| {
            v.map(str::to_owned).or_else(|| had.and_then(had_v))
        };
        let halted_sound = sound(runners.halted_sound, |d| d.halted_sound.clone());
        let resumed_sound = sound(runners.resumed_sound, |d| d.resumed_sound.clone());
        self.list.retain(|d| d.token != token);
        let clip = |s: &str| {
            s.chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>()
        };
        self.list.push(Device {
            token: token.to_owned(),
            platform: clip(platform),
            name: clip(name),
            registered_at: now,
            runners_new,
            runners_breakout,
            runners_burst,
            runners_running,
            runners_run,
            runners_fast,
            runners_shelf,
            runners_follow_up,
            ross_crossing,
            signals_crossing,
            ross_trigger,
            signals_trigger,
            halt_watch,
            runners_pullback,
            drawing_tones,
            halted,
            resumed,
            halted_sound,
            resumed_sound,
            halt_sounds,
            n1_shadow,
        });
        while self.list.len() > MAX_DEVICES {
            self.list.remove(0);
        }
        self.save();
        true
    }

    pub fn remove(&mut self, token: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|d| d.token != token);
        let removed = self.list.len() != before;
        if removed {
            self.save();
        }
        removed
    }
}

/// One notification for every registered phone.
#[derive(Clone, Debug)]
pub struct Job {
    pub event_id: String,
    pub tokens: Vec<String>,
    pub title: String,
    pub body: String,
    pub data: Value,
    /// A Runners push (`super::runners`): sent at once, never held for the batch window, and its
    /// latency logged once Expo took it.
    pub runner: Option<RunnerLatency>,
    /// The Android notification channel when not the plain alerts one: a sounding drawing
    /// alert's `drawing-up` / `drawing-down` / `drawing-stop` (apps/mobile src/notifications
    /// setup.ts creates them, each with its own bundled tone; a channel's sound is fixed on
    /// Android, so the tone IS the channel), or a halt sound's `halt-*` channel ([`halt_channel`]).
    pub channel: Option<&'static str>,
    /// The push's `sound` when not the platform default: a halt push to a phone with the halt
    /// sound channels names its bundled file for iOS ([`halt_ios_sound`]); `Some(None)` = silent.
    pub sound: Option<Option<&'static str>>,
}

/// A Runners push's clock (`super::runners`, "Latency"): epoch ns.
#[derive(Clone, Debug)]
pub struct RunnerLatency {
    pub symbol: String,
    pub kind: &'static str,
    /// The data kind the clients see (`runners::data_kind`), for the ledger.
    pub notice: &'static str,
    /// The triggering print's execution time, when the source knows it (a "Run +20%" row).
    pub exec_ns: Option<i64>,
    /// When the board decided (the row's time: the market clock, the wall clock live).
    pub decided_ns: i64,
    /// When the alerts task handed it to this task.
    pub queued_ns: i64,
}

/// What the push task reports back to the alerts task.
#[derive(Clone, Debug, PartialEq)]
pub enum PushResult {
    /// The push service did not take this event's notification (after the retries).
    Failed { event_id: String, detail: String },
    /// The service says this phone no longer exists (uninstalled, token rotated).
    Unregistered { token: String },
}

#[derive(Clone, Debug)]
pub struct PushConfig {
    pub base: String,
    pub access_token: Option<String>,
}

impl PushConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        PushConfig {
            base: var("EXPO_PUSH_BASE")
                .unwrap_or_else(|| DEFAULT_BASE.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            access_token: var("EXPO_ACCESS_TOKEN"),
        }
    }
}

fn client() -> Result<reqwest::Client, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_platform_verifier()
        .map_err(|e| format!("tls verifier: {e}"))?
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("stockwatcher-server/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Starts the push task. Jobs go in through the returned sender (bounded: a full queue drops
/// the job, which the caller records as failed); results come out on `results`.
pub fn spawn(
    cfg: PushConfig,
    results: mpsc::Sender<PushResult>,
    ledger: super::ledger::Ledger,
) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel(256);
    match client() {
        Ok(http) => {
            tokio::spawn(run(http, cfg, rx, results, ledger));
        }
        Err(e) => {
            tracing::error!(error = %e, "push: no HTTP client; phone notifications are off (in-app alerts still work)");
            // Every job fails at once rather than queueing forever.
            tokio::spawn(async move {
                let mut rx = rx;
                while let Some(job) = rx.recv().await {
                    let _ = results
                        .send(PushResult::Failed {
                            event_id: job.event_id,
                            detail: "no HTTP client".into(),
                        })
                        .await;
                }
            });
        }
    }
    tx
}

struct Ticket {
    token: String,
    sent: tokio::time::Instant,
}

async fn post(
    http: &reqwest::Client,
    cfg: &PushConfig,
    path: &str,
    body: &Value,
) -> Result<Value, (bool, String)> {
    let mut req = http
        .post(format!("{}{path}", cfg.base))
        .header("content-type", "application/json")
        .header("accept", "application/json");
    if let Some(t) = &cfg.access_token {
        req = req.bearer_auth(t);
    }
    let resp = req
        .body(serde_json::to_vec(body).expect("json"))
        .send()
        .await
        .map_err(|e| (true, e.without_url().to_string()))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| (true, e.without_url().to_string()))?;
    if status.as_u16() == 429 || status.is_server_error() {
        return Err((true, format!("HTTP {status}")));
    }
    if !status.is_success() {
        return Err((
            false,
            format!(
                "HTTP {status}: {}",
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| (false, format!("bad response: {e}")))
}

fn device_gone(v: &Value) -> bool {
    v.pointer("/details/error").and_then(Value::as_str) == Some("DeviceNotRegistered")
}

async fn run(
    http: reqwest::Client,
    cfg: PushConfig,
    mut rx: mpsc::Receiver<Job>,
    results: mpsc::Sender<PushResult>,
    ledger: super::ledger::Ledger,
) {
    let mut tickets: HashMap<String, Ticket> = HashMap::new();
    let mut receipts = tokio::time::interval(RECEIPT_CHECK);
    receipts.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            job = rx.recv() => {
                let Some(first) = job else { break };
                // Batch: whatever else arrives within the window, up to the request limit. A
                // Runners push does not wait for the window (seconds matter on a +20 % leg): it
                // goes with whatever is already queued, so a single urgent push leaves at once
                // and a burst of them (queued together by one board pass) still shares one
                // request. A Runners push that arrives while a price alert's window is open ends
                // the window: it would otherwise wait up to BATCH_WINDOW behind that alert.
                let mut urgent = first.runner.is_some();
                let mut jobs = vec![first];
                let deadline = tokio::time::Instant::now() + BATCH_WINDOW;
                while jobs.iter().map(|j| j.tokens.len()).sum::<usize>() < MAX_BATCH {
                    if urgent {
                        match rx.try_recv() {
                            Ok(j) => jobs.push(j),
                            Err(_) => break,
                        }
                        continue;
                    }
                    match tokio::time::timeout_at(deadline, rx.recv()).await {
                        Ok(Some(j)) => {
                            urgent = j.runner.is_some();
                            jobs.push(j);
                        }
                        _ => break,
                    }
                }
                let lat: Vec<(String, RunnerLatency)> = jobs
                    .iter()
                    .filter_map(|j| j.runner.clone().map(|l| (j.event_id.clone(), l)))
                    .collect();
                send_batch(&http, &cfg, jobs, &mut tickets, &results).await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as i64);
                for (event_id, l) in lat {
                    // The notification ledger keeps it past a deploy (super::ledger, `pushed`).
                    ledger.pushed(&event_id, l.notice, &l.symbol, l.exec_ns, l.decided_ns, l.queued_ns, now);
                    let ms = |a: i64, b: i64| (b - a) as f64 / 1e6;
                    tracing::info!(
                        symbol = %l.symbol,
                        kind = l.kind,
                        exec_ns = l.exec_ns,
                        decided_ns = l.decided_ns,
                        queued_ns = l.queued_ns,
                        expo_ns = now,
                        trade_to_board_ms = l.exec_ns.map(|e| ms(e, l.decided_ns)),
                        board_to_queue_ms = ms(l.decided_ns, l.queued_ns),
                        queue_to_expo_ms = ms(l.queued_ns, now),
                        trade_to_expo_ms = l.exec_ns.map(|e| ms(e, now)),
                        "runners push: latency"
                    );
                }
            }
            _ = receipts.tick() => check_receipts(&http, &cfg, &mut tickets, &results).await,
        }
    }
}

/// One Expo push message: `job` to the device `token`.
fn expo_message(job: &Job, token: &str) -> Value {
    let mut m = json!({
        "to": token,
        "title": job.title,
        "body": job.body,
        "data": job.data,
        "sound": match job.sound {
            None => Value::from("default"),
            Some(Some(file)) => Value::from(file),
            Some(None) => Value::Null,
        },
        "priority": "high",
        // The channel the app creates (apps/mobile/src/notifications/setup.ts ALERTS_CHANNEL_ID,
        // or a drawing alert's tone channel).
        "channelId": job.channel.unwrap_or("alerts"),
    });
    // A Runners push gets the phone's "Copy ticker" button (super::runners::CATEGORY_ID). Expo
    // forwards `categoryId` to Android in the FCM data (expo-notifications NotificationData reads
    // `categoryId`) and to iOS as the APNs category. A price alert's push has no button.
    if let Some(r) = &job.runner {
        m["categoryId"] = Value::from(super::runners::CATEGORY_ID);
        // Every distinct event is its own push (super::runners, "Every distinct event is
        // delivered"), so a burst of them on one stock stacks under that stock instead of
        // scattering: `threadId` (iOS, APNs `thread-id`) groups them, never replaces one. Expo's
        // push API has no Android group field (Android bundles an app's notifications itself);
        // `tag` and `collapseId` are deliberately NOT set: both replace an earlier notification.
        m["threadId"] = Value::from(format!("runners-{}", r.symbol));
    }
    m
}

async fn send_batch(
    http: &reqwest::Client,
    cfg: &PushConfig,
    jobs: Vec<Job>,
    tickets: &mut HashMap<String, Ticket>,
    results: &mpsc::Sender<PushResult>,
) {
    // One message per (job, token), in order, so ticket i belongs to message i.
    let mut msgs = Vec::new();
    let mut owner: Vec<(usize, String)> = Vec::new();
    for (j, job) in jobs.iter().enumerate() {
        for t in &job.tokens {
            msgs.push(expo_message(job, t));
            owner.push((j, t.clone()));
        }
    }
    for chunk_start in (0..msgs.len()).step_by(MAX_BATCH) {
        let chunk = &msgs[chunk_start..(chunk_start + MAX_BATCH).min(msgs.len())];
        let body = Value::Array(chunk.to_vec());
        let mut res = post(http, cfg, "/push/send", &body).await;
        for wait in RETRY_WAITS {
            match &res {
                Err((true, e)) => {
                    tracing::warn!(error = %e, retry_in = ?wait, "push: send failed; retrying");
                    tokio::time::sleep(wait).await;
                    res = post(http, cfg, "/push/send", &body).await;
                }
                _ => break,
            }
        }
        match res {
            Ok(v) => {
                let data = v
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for (i, ticket) in data.iter().enumerate() {
                    let Some((j, token)) = owner.get(chunk_start + i) else {
                        continue;
                    };
                    if ticket.get("status").and_then(Value::as_str) == Some("ok") {
                        if let Some(id) = ticket.get("id").and_then(Value::as_str) {
                            tickets.insert(
                                id.to_owned(),
                                Ticket {
                                    token: token.clone(),
                                    sent: tokio::time::Instant::now(),
                                },
                            );
                        }
                    } else if device_gone(ticket) {
                        let _ = results
                            .send(PushResult::Unregistered {
                                token: token.clone(),
                            })
                            .await;
                    } else {
                        let detail = ticket
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("refused")
                            .to_owned();
                        tracing::warn!(detail, "push: ticket error");
                        let _ = results
                            .send(PushResult::Failed {
                                event_id: jobs[*j].event_id.clone(),
                                detail,
                            })
                            .await;
                    }
                }
            }
            Err((_, e)) => {
                tracing::warn!(error = %e, "push: gave up on a batch; the alerts reached the app only");
                let mut failed: Vec<&str> = owner
                    [chunk_start..(chunk_start + MAX_BATCH).min(owner.len())]
                    .iter()
                    .map(|(j, _)| jobs[*j].event_id.as_str())
                    .collect();
                failed.dedup();
                for id in failed {
                    let _ = results
                        .send(PushResult::Failed {
                            event_id: id.to_owned(),
                            detail: e.clone(),
                        })
                        .await;
                }
            }
        }
    }
}

async fn check_receipts(
    http: &reqwest::Client,
    cfg: &PushConfig,
    tickets: &mut HashMap<String, Ticket>,
    results: &mpsc::Sender<PushResult>,
) {
    // Expo keeps receipts for a day; anything older is gone either way.
    tickets.retain(|_, t| t.sent.elapsed() < Duration::from_secs(24 * 3600));
    let due: Vec<String> = tickets
        .iter()
        .filter(|(_, t)| t.sent.elapsed() >= RECEIPT_DELAY)
        .map(|(id, _)| id.clone())
        .take(1000)
        .collect();
    if due.is_empty() {
        return;
    }
    match post(http, cfg, "/push/getReceipts", &json!({ "ids": due })).await {
        Ok(v) => {
            for id in &due {
                let Some(t) = tickets.remove(id) else {
                    continue;
                };
                if let Some(r) = v.pointer(&format!("/data/{id}")) {
                    if device_gone(r) {
                        let _ = results
                            .send(PushResult::Unregistered { token: t.token })
                            .await;
                    } else if r.get("status").and_then(Value::as_str) == Some("error") {
                        tracing::warn!(detail = ?r.get("message"), "push: delivery receipt error");
                    }
                }
            }
        }
        Err((_, e)) => tracing::warn!(error = %e, "push: receipts check failed; will retry"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post as axum_post;
    use axum::{Json, Router};
    use std::sync::Mutex;

    #[test]
    fn tokens_and_devices() {
        assert!(valid_token("ExponentPushToken[abcDEF123]"));
        assert!(valid_token("ExpoPushToken[x]"));
        assert!(!valid_token("ExponentPushToken[abc"));
        assert!(!valid_token("fcm:abc"));
        let dir = tempfile::tempdir().unwrap();
        let mut d = Devices::open(dir.path());
        assert!(!d.register("nope", "android", "Fold", 1, RunnersOptIn::default()));
        for i in 0..12 {
            assert!(d.register(
                &format!("ExponentPushToken[{i}]"),
                "android",
                "Fold\u{0}7",
                i,
                RunnersOptIn::default()
            ));
        }
        assert_eq!(d.len(), MAX_DEVICES, "the oldest go first");
        let back = Devices::open(dir.path());
        assert_eq!(back.tokens().first().unwrap(), "ExponentPushToken[2]");
        assert_eq!(back.list[0].name, "Fold7", "control characters dropped");
        assert!(d.remove("ExponentPushToken[5]"));
        assert!(!d.remove("ExponentPushToken[5]"));
    }

    /// The Runners opt-ins: defaults for a new phone, kept when an older app re-registers
    /// without them, changed when sent, persisted, and read from a file written before they
    /// existed.
    #[test]
    fn runners_opt_ins_round_trip() {
        use crate::rocket::NoticeKind;
        let dir = tempfile::tempdir().unwrap();
        let mut d = Devices::open(dir.path());
        let fold = "ExponentPushToken[fold]";
        assert!(d.register(fold, "android", "Fold", 1, RunnersOptIn::default()));
        assert_eq!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::New, "A")),
            vec![fold.to_string()]
        );
        assert!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Breakout, "A"))
                .is_empty()
        );
        // Bursts are on by default.
        assert_eq!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Burst, "A")),
            vec![fold.to_string()]
        );
        // Running and "Run +20%" entries are on by default (owner, 2026-10-22).
        assert_eq!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Running, "A")),
            vec![fold.to_string()]
        );
        assert_eq!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Run, "A")),
            vec![fold.to_string()]
        );
        // So are "Fast move" entries.
        assert_eq!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Fast, "A")),
            vec![fold.to_string()]
        );
        let both = RunnersOptIn {
            new: Some(false),
            breakout: Some(true),
            burst: Some(false),
            running: Some(false),
            run: Some(false),
            fast: Some(false),
            ..RunnersOptIn::default()
        };
        assert!(d.register(fold, "android", "Fold", 2, both));
        assert!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::New, "A"))
                .is_empty()
        );
        assert_eq!(
            d.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Breakout, "A")),
            vec![fold.to_string()]
        );
        // An older app's register (no opt-ins) keeps them.
        assert!(d.register(fold, "android", "Fold", 3, RunnersOptIn::default()));
        let back = Devices::open(dir.path());
        let dev = back.get(fold).unwrap();
        assert!(!dev.runners_new && dev.runners_breakout && !dev.runners_burst);
        assert!(!dev.runners_running, "kept from the register before");
        assert!(!dev.runners_run, "kept from the register before");
        assert!(!dev.runners_fast, "kept from the register before");
        assert!(
            back.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Fast, "A"))
                .is_empty()
        );
        assert!(
            back.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Burst, "A"))
                .is_empty()
        );
        // A file from before the opt-ins existed: new flags on, breakouts off.
        std::fs::write(
            dir.path().join(FILE),
            r#"[{"token":"ExponentPushToken[old]","registeredAt":1}]"#,
        )
        .unwrap();
        let old = Devices::open(dir.path());
        assert_eq!(
            old.runner_tokens(&crate::rocket::Notice::test(NoticeKind::New, "A"))
                .len(),
            1
        );
        assert!(
            old.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Breakout, "A"))
                .is_empty()
        );
        assert_eq!(
            old.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Burst, "A"))
                .len(),
            1
        );
        assert!(
            old.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Running, "A"))
                .is_empty()
        );
        // A record without the "Run +20%" field: on.
        assert_eq!(
            old.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Run, "A"))
                .len(),
            1
        );
        // A record without the "Fast move" field: on.
        assert_eq!(
            old.runner_tokens(&crate::rocket::Notice::test(NoticeKind::Fast, "A"))
                .len(),
            1
        );
    }

    /// The panel kinds (owner, 2026-10-23): crossings, shelf entries and follow-ups on, "At
    /// trigger" and the Halt watcher off, for a new phone AND a record saved before they existed;
    /// an older app's register keeps what the phone chose; a Ross crossing on both panels
    /// reaches a phone through either switch.
    #[test]
    fn panel_opt_ins_defaults_and_old_records() {
        use crate::rocket::{CueNotice, Notice, NoticeDetail, NoticeKind as K};
        // The pullback stages too: on for a new phone AND for a record saved before they existed.
        let on = [
            K::Shelf,
            K::FollowUp,
            K::RossCrossing,
            K::SignalsCrossing,
            K::Pullback,
            K::PullbackCrossing,
        ];
        let off = [K::RossTrigger, K::SignalsTrigger, K::Halt];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            r#"[{"token":"ExponentPushToken[old]","registeredAt":1,"runnersFast":false}]"#,
        )
        .unwrap();
        let mut d = Devices::open(dir.path());
        let fresh = "ExponentPushToken[new]";
        assert!(d.register(fresh, "android", "Fold", 2, RunnersOptIn::default()));
        for k in on {
            assert_eq!(d.runner_tokens(&Notice::test(k, "A")).len(), 2, "{k:?} on");
        }
        for k in off {
            assert!(
                d.runner_tokens(&Notice::test(k, "A")).is_empty(),
                "{k:?} off"
            );
        }
        // The phone turns Ross crossings off and the Halt watcher on; an older app's register
        // then keeps both.
        let more: MoreOptIn = serde_json::from_str(
            r#"{"rossCrossing":false,"haltWatch":true,"runnersShelf":false,"runnersPullback":false}"#,
        )
        .unwrap();
        assert!(d.register(
            fresh,
            "android",
            "Fold",
            3,
            more.with(RunnersOptIn::default())
        ));
        assert!(d.register(fresh, "android", "Fold", 4, RunnersOptIn::default()));
        let dev = Devices::open(dir.path()).get(fresh).unwrap().clone();
        assert!(!dev.ross_crossing && dev.halt_watch && !dev.runners_shelf);
        assert!(
            !dev.runners_pullback,
            "the phone's pullback switch off is kept"
        );
        assert!(dev.signals_crossing && dev.runners_follow_up && !dev.ross_trigger);
        // A Ross crossing whose episode Signals shows too: this phone (Signals on) still gets it.
        let mut n = Notice::test(K::RossCrossing, "A");
        n.detail = Some(NoticeDetail::Cue(CueNotice {
            episode_id: "e".into(),
            trigger: Some(2.4),
            also_signals: true,
        }));
        assert_eq!(d.runner_tokens(&n).len(), 2);
        n.detail = Some(NoticeDetail::Cue(CueNotice {
            episode_id: "e".into(),
            trigger: Some(2.4),
            also_signals: false,
        }));
        assert_eq!(
            d.runner_tokens(&n),
            vec!["ExponentPushToken[old]".to_string()]
        );
        // The desk's own spellings parse into the same switches.
        let desk: MoreOptIn =
            serde_json::from_str(r#"{"shelf":false,"followUp":false,"halt":true}"#).unwrap();
        let k = crate::alerts::DeskKinds::from_opt_in(desk.with(RunnersOptIn::default()));
        assert!(!k.shelf && !k.follow_up && k.halt && k.ross_crossing && !k.ross_trigger);
    }

    /// A stub of Expo's two endpoints: records batches; the second token is "not registered".
    #[tokio::test]
    async fn batches_and_unregistered_devices() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s2 = seen.clone();
        let app = Router::new().route(
            "/push/send",
            axum_post(move |Json(body): Json<Value>| {
                let s2 = s2.clone();
                async move {
                    let msgs = body.as_array().cloned().unwrap_or_default();
                    s2.lock().unwrap().push(body);
                    let data: Vec<Value> = msgs
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            if m["to"] == "ExponentPushToken[gone]" {
                                json!({"status": "error", "message": "not registered", "details": {"error": "DeviceNotRegistered"}})
                            } else {
                                json!({"status": "ok", "id": format!("t{i}")})
                            }
                        })
                        .collect();
                    Json(json!({ "data": data }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (rtx, mut rrx) = mpsc::channel(8);
        let tx = spawn(
            PushConfig {
                base: format!("http://{addr}"),
                access_token: None,
            },
            rtx,
            super::super::ledger::Ledger::default(),
        );
        for n in 0..3 {
            tx.send(Job {
                event_id: format!("e{n}"),
                tokens: vec![
                    "ExponentPushToken[ok]".into(),
                    "ExponentPushToken[gone]".into(),
                ],
                title: "XLAB".into(),
                body: "Crossed".into(),
                data: json!({}),
                runner: None,
                channel: None,
                sound: None,
            })
            .await
            .unwrap();
        }
        let r = tokio::time::timeout(Duration::from_secs(5), rrx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            r,
            PushResult::Unregistered {
                token: "ExponentPushToken[gone]".into()
            }
        );
        let batches = seen.lock().unwrap().clone();
        assert_eq!(
            batches.len(),
            1,
            "three jobs inside the window go out in one request"
        );
        assert_eq!(batches[0].as_array().unwrap().len(), 6);
        assert_eq!(batches[0][0]["channelId"], "alerts");
    }

    /// A Runners push never sits in the batch window: alone it leaves at once, and one that
    /// arrives while a price alert's window is open takes that alert with it right away instead
    /// of waiting out the window behind it.
    #[tokio::test]
    async fn a_runners_push_does_not_wait_for_the_batch_window() {
        let seen: Arc<Mutex<Vec<(std::time::Instant, usize)>>> = Arc::default();
        let s2 = seen.clone();
        let app = Router::new().route(
            "/push/send",
            axum_post(move |Json(body): Json<Value>| {
                let s2 = s2.clone();
                async move {
                    let msgs = body.as_array().cloned().unwrap_or_default();
                    s2.lock()
                        .unwrap()
                        .push((std::time::Instant::now(), msgs.len()));
                    let data: Vec<Value> = (0..msgs.len())
                        .map(|i| json!({"status": "ok", "id": format!("t{i}")}))
                        .collect();
                    Json(json!({ "data": data }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (rtx, _rrx) = mpsc::channel(8);
        let tx = spawn(
            PushConfig {
                base: format!("http://{addr}"),
                access_token: None,
            },
            rtx,
            super::super::ledger::Ledger::default(),
        );
        let job = |id: &str, runner: bool| Job {
            event_id: id.into(),
            tokens: vec!["ExponentPushToken[ok]".into()],
            title: "XLAB".into(),
            body: "b".into(),
            data: json!({}),
            runner: runner.then(|| RunnerLatency {
                symbol: "XLAB".into(),
                kind: "fast",
                notice: "fast",
                exec_ns: None,
                decided_ns: 0,
                queued_ns: 0,
            }),
            channel: None,
            sound: None,
        };
        let wait_for = |n: usize| {
            let seen = seen.clone();
            async move {
                for _ in 0..500 {
                    if seen.lock().unwrap().len() >= n {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                panic!("no request {n}");
            }
        };
        // Alone.
        let t0 = std::time::Instant::now();
        tx.send(job("r1", true)).await.unwrap();
        wait_for(1).await;
        let alone = seen.lock().unwrap()[0].0 - t0;
        // Behind a price alert, 30 ms into its window.
        let t1 = std::time::Instant::now();
        tx.send(job("p1", false)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(job("r2", true)).await.unwrap();
        wait_for(2).await;
        let (at, n) = seen.lock().unwrap()[1];
        let behind = at - t1;
        assert_eq!(n, 2, "the price alert goes with it, one request");
        // The window is 300 ms; local HTTP to the stub takes a few.
        let bound = BATCH_WINDOW / 2;
        assert!(alone < bound, "alone: {alone:?}");
        assert!(behind < bound, "behind a price alert: {behind:?}");
    }

    /// Halt pushes (plan "halt sounds", 2026-11-16): on for a new phone and an old record; an
    /// older app's register keeps the phone's choices; an unknown sound key is ignored; only a
    /// phone with the halt sound channels (release 2) gets its sound's channel and iOS file.
    #[test]
    fn halt_opt_ins_sounds_and_channels() {
        use crate::rocket::{Notice, NoticeKind as K};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            r#"[{"token":"ExponentPushToken[old]","registeredAt":1}]"#,
        )
        .unwrap();
        let mut d = Devices::open(dir.path());
        for k in [K::Halted, K::Resumed] {
            assert_eq!(d.runner_tokens(&Notice::test(k, "A")).len(), 1, "{k:?} on");
        }
        let fresh = "ExponentPushToken[new]";
        let more: MoreOptIn = serde_json::from_str(
            r#"{"resumed":false,"haltedSound":"ding","resumedSound":"bogus","haltSounds":true}"#,
        )
        .unwrap();
        assert_eq!(more.resumed_sound, None, "an unknown key is ignored");
        assert!(d.register(
            fresh,
            "android",
            "Fold",
            2,
            more.with(RunnersOptIn::default())
        ));
        // An older app registers again: nothing about halts changes.
        assert!(d.register(fresh, "android", "Fold", 3, RunnersOptIn::default()));
        let d = Devices::open(dir.path());
        let dev = d.get(fresh).unwrap().clone();
        assert!(dev.halted && !dev.resumed && dev.halt_sounds);
        assert_eq!(dev.halted_sound.as_deref(), Some("ding"));
        assert!(d.runner_tokens(&Notice::test(K::Resumed, "A")).len() == 1);
        let mut groups = d.halt_groups(&Notice::test(K::Halted, "A"));
        groups.sort_by(|a, b| a.2.cmp(&b.2));
        assert_eq!(
            groups,
            vec![
                ("ding", true, vec![fresh.to_string()]),
                ("siren", false, vec!["ExponentPushToken[old]".to_string()]),
            ]
        );
        let job = |channels: bool, key: &'static str| Job {
            event_id: "runners-halted-A-halt-1".into(),
            tokens: vec![],
            title: "A · Halted · $2.00".into(),
            body: "LULD pause".into(),
            data: json!({"symbol": "A", "kind": "halt.halted", "sound": key}),
            runner: None,
            channel: channels.then(|| halt_channel(key)),
            sound: channels.then(|| halt_ios_sound(key)),
        };
        let m = expo_message(&job(false, "siren"), "ExponentPushToken[old]");
        assert_eq!(
            (m["channelId"].clone(), m["sound"].clone()),
            (json!("alerts"), json!("default"))
        );
        assert_eq!(m["data"]["sound"], "siren");
        let m = expo_message(&job(true, "ding"), fresh);
        assert_eq!(
            (m["channelId"].clone(), m["sound"].clone()),
            (json!("halt-ding-v1"), json!("halt_ding.wav"))
        );
        let m = expo_message(&job(true, "none"), fresh);
        assert_eq!(
            (m["channelId"].clone(), m["sound"].clone()),
            (json!("halt-none-v1"), Value::Null)
        );
        for k in super::super::halts::SOUNDS {
            assert!(halt_channel(k).starts_with("halt-"));
            assert_eq!(halt_ios_sound(k).is_none(), k == "none");
        }
    }

    #[test]
    fn runners_pushes_carry_the_copy_ticker_category() {
        let n = crate::rocket::Notice {
            kind: crate::rocket::NoticeKind::Fast,
            alert_id: "2026-10-23-YMAT-1".into(),
            symbol: "YMAT".into(),
            at_ns: 0,
            price: 2.41,
            from_flag_pct: None,
            reasons: vec![],
            since_start_min: None,
            exec_ns: None,
            detail: None,
        };
        let (title, body, data) = super::super::runners::message(&n);
        let mut job = Job {
            event_id: "runners-fast-1".into(),
            tokens: vec!["ExponentPushToken[a]".into()],
            title,
            body,
            data,
            runner: Some(RunnerLatency {
                symbol: "YMAT".into(),
                kind: "fast",
                notice: "runners.fast",
                exec_ns: None,
                decided_ns: 0,
                queued_ns: 0,
            }),
            channel: None,
            sound: None,
        };
        let m = expo_message(&job, "ExponentPushToken[a]");
        assert_eq!(m["categoryId"], "runners");
        // Grouped by stock on iOS; nothing that would replace an earlier notification.
        assert_eq!(m["threadId"], "runners-YMAT");
        assert!(m.get("tag").is_none() && m.get("collapseId").is_none());
        assert_eq!(m["channelId"], "alerts");
        assert_eq!(m["data"]["symbol"], "YMAT");
        assert!(
            m["body"]
                .as_str()
                .unwrap()
                .ends_with("Alert $2.41 · +5% = $2.53")
        );
        // A price alert's push: no button.
        job.runner = None;
        let m = expo_message(&job, "ExponentPushToken[a]");
        assert!(m.get("categoryId").is_none() && m.get("threadId").is_none());
        // A sounding drawing alert's push goes out on its tone's channel.
        job.channel = Some("drawing-stop");
        assert_eq!(
            expo_message(&job, "ExponentPushToken[a]")["channelId"],
            "drawing-stop"
        );
    }

    #[tokio::test]
    async fn an_unreachable_service_fails_the_event() {
        // Nothing listens on this port: connection refused on every try (the waits are real, so
        // this takes ~14 s; kept because "Expo down" is the case that matters).
        let (rtx, mut rrx) = mpsc::channel(8);
        let tx = spawn(
            PushConfig {
                base: "http://127.0.0.1:9".into(),
                access_token: None,
            },
            rtx,
            super::super::ledger::Ledger::default(),
        );
        tx.send(Job {
            event_id: "e1".into(),
            tokens: vec!["ExponentPushToken[ok]".into()],
            title: "t".into(),
            body: "b".into(),
            data: json!({}),
            runner: None,
            channel: None,
            sound: None,
        })
        .await
        .unwrap();
        let r = tokio::time::timeout(Duration::from_secs(30), rrx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(r, PushResult::Failed { event_id, .. } if event_id == "e1"));
    }
}
