//! Synced settings: the user's chart preferences and drawings, shared live by every client
//! (owner request 2026-10-18: "if I change one indicator on the desktop/web, it should change
//! everywhere else live and the change should persist"). Design and the synced vs device-local
//! table: docs/architecture/settings-sync.md. Wire messages: the top of ws.rs.
//!
//! A small document store, typed but opaque: a namespace (`layers`, `chartSettings`,
//! `drawings:SMPL`, …) holds one JSON value with a per-namespace `rev` (1, 2, 3, … bumped on every
//! applied write) and `updatedAt` (wall-clock epoch ms). The server never interprets a value:
//! the clients own the schemas and validate everything they read (chart-core's parse functions),
//! so the server only checks names and sizes. That keeps a client-side settings change (a new
//! chart setting, a new indicator field) from ever needing a server release.
//!
//! Concurrency: last writer wins, in server arrival order, with no compare-and-set. It is a
//! single-user app (one token), so two writes to the same namespace are the same person on two
//! devices, and the one that reached the server last is that person's latest action. A
//! compare-and-set would refuse it and make the client rebase and retry, which can only lose or
//! delay what the user just did. Every applied write is broadcast (`settings.changed`, with the
//! new rev and the writer's `opId`) to EVERY connection, the writer included; the writer uses the
//! `opId` to confirm its own write, the others apply any rev newer than theirs. Nothing writes
//! in response to a broadcast, so nothing can ping-pong.
//!
//! Granularity: a whole-value `set` for preference documents, and an entry-level `patch` (set
//! and remove keys of an object-valued document) for maps such as a symbol's drawings, keyed by
//! drawing id. With a whole-list set, two devices editing DIFFERENT drawings of the same symbol
//! at once would lose one edit to LWW; with entry patches both land and only an edit of the SAME
//! drawing is last-writer-wins.
//!
//! Persistence: one file per namespace under `DATA_DIR/settings/` (the persistent volume on the
//! VPS: `stockwatcher_data` at /app/data). Per namespace rather than one file because writes are
//! per namespace and frequent (every drawing drop rewrites only that symbol's file, not every
//! symbol's drawings), and a failed write can only ever affect the one namespace it was for.
//! Each write is temp file + fsync + rename (+ directory fsync on Unix), so a crash leaves either
//! the old or the new version, never a torn one. A write is durable BEFORE it is applied in
//! memory and broadcast: a client that saw its write confirmed can rely on it surviving a
//! restart or a redeploy. File names are the namespace in hex: namespaces may contain `:` (not
//! allowed in Windows file names) and differ only in case (`drawings:abc` vs `drawings:ABC`,
//! which collide on case-insensitive file systems). Each file also holds its namespace in clear
//! text, for a human and for the loader's cross-check.
//!
//! Independent of the market: a replay seek or day switch, live mode and the archive never touch
//! it, and it is not part of the archive.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{Mutex, broadcast};

/// Namespace names: 1..=80 of `[A-Za-z0-9._:-]`, not starting with `.`.
pub const MAX_NS_LEN: usize = 80;
/// A client's operation id, echoed back in `settings.changed` / `settings.rejected`.
pub const MAX_OP_ID_LEN: usize = 64;
/// Keys of a patched (map) document, e.g. a drawing id.
pub const MAX_KEY_LEN: usize = 128;
/// One document's serialized value. A whole-value `set` is further bounded by the WebSocket's
/// 64 KiB message limit (ws.rs MAX_MESSAGE_BYTES); patched maps can grow past that, up to here.
pub const MAX_DOC_BYTES: usize = 256 * 1024;
/// Every document together. Generous for preferences and drawings (a drawing is ~200 bytes),
/// small enough that the snapshot every client gets on connect stays cheap.
pub const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// How many namespaces may exist (each symbol with drawings is one).
pub const MAX_DOCS: usize = 2000;
/// Broadcast backlog per connection; a connection that falls further behind gets a fresh
/// `settings.snapshot` instead (it may have missed a change).
const CHANNEL: usize = 256;

/// A write the server refused. `Unavailable` (the disk write failed) is the only one worth
/// retrying: the clients keep that edit and send it again after the next reconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    BadNamespace,
    BadOpId,
    BadKey,
    TooLarge,
    StoreFull,
    NotAMap,
    Unavailable,
}

impl SettingsError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadNamespace | Self::BadOpId | Self::BadKey => "bad_request",
            Self::TooLarge => "too_large",
            Self::StoreFull => "store_full",
            Self::NotAMap => "not_a_map",
            Self::Unavailable => "settings_unavailable",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::BadNamespace => "invalid settings namespace",
            Self::BadOpId => "invalid opId",
            Self::BadKey => "invalid map key",
            Self::TooLarge => "settings value too large",
            Self::StoreFull => "settings store is full",
            Self::NotAMap => "patch on a document that is not an object",
            Self::Unavailable => "settings could not be saved; try again later",
        }
    }
}

/// One write. `Set` replaces the value; with `if_absent` it applies only when the namespace has
/// never been written (the clients' one-time migration of their pre-sync local settings: the
/// first client to connect seeds the shared value, later ones adopt it). `Patch` sets and
/// removes keys of an object-valued document (absent = `{}`).
#[derive(Debug, Clone)]
pub enum SettingsOp {
    Set {
        value: Value,
        if_absent: bool,
    },
    Patch {
        set: Map<String, Value>,
        remove: Vec<String>,
    },
}

#[derive(Debug, Clone)]
struct Doc {
    rev: u64,
    updated_at: u64,
    value: Value,
    /// Serialized size of `value`, counted against the limits.
    bytes: usize,
}

/// A document as written to disk (and as listed in a snapshot, minus `type`).
#[derive(Serialize, Deserialize)]
struct Stored {
    ns: String,
    rev: u64,
    #[serde(rename = "updatedAt")]
    updated_at: u64,
    value: Value,
}

#[derive(Default)]
struct Docs {
    map: BTreeMap<String, Doc>,
    total: usize,
}

struct Inner {
    /// None = memory only (tests, and AppState::new).
    dir: Option<PathBuf>,
    /// Async: a write holds it across the disk write, so applies, persists and broadcasts happen
    /// strictly one at a time and in the same order (revs go out in order on the channel).
    docs: Mutex<Docs>,
    tx: broadcast::Sender<Arc<str>>,
}

/// The store. Cheap to clone (shared).
#[derive(Clone)]
pub struct Settings(Arc<Inner>);

pub fn valid_namespace(ns: &str) -> bool {
    !ns.is_empty()
        && ns.len() <= MAX_NS_LEN
        && !ns.starts_with('.')
        && ns
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

fn valid_op_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_OP_ID_LEN && id.bytes().all(|b| b.is_ascii_graphic())
}

/// `<namespace as lowercase hex>.json` (see the module comment for why hex).
fn file_name(ns: &str) -> String {
    let mut s = String::with_capacity(ns.len() * 2 + 5);
    for b in ns.bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s.push_str(".json");
    s
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn json_len(v: &Value) -> usize {
    serde_json::to_string(v)
        .map(|s| s.len())
        .unwrap_or(usize::MAX)
}

/// Temp file, fsync, rename over the old file, fsync the directory (Unix; Windows has no
/// directory handle to sync and NTFS journals the rename).
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!("{name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(name))?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

impl Settings {
    /// Memory only: nothing survives the process. For tests and servers built without a data dir.
    pub fn in_memory() -> Self {
        Self::build(None, Docs::default())
    }

    fn build(dir: Option<PathBuf>, docs: Docs) -> Self {
        let (tx, _) = broadcast::channel(CHANNEL);
        Self(Arc::new(Inner {
            dir,
            docs: Mutex::new(docs),
            tx,
        }))
    }

    /// Opens (creating it if needed) the store in `dir` and loads every document. A leftover
    /// `*.tmp` is a write that crashed before its rename, so the `.json` beside it still holds
    /// the last complete version: it is deleted. A file that does not parse, or whose name does
    /// not match its namespace, is renamed to `*.corrupt` (kept for a human, never overwritten
    /// silently) and skipped. Only an unusable directory is an error.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let mut docs = Docs::default();
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            if name.ends_with(".tmp") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !name.ends_with(".json") {
                continue;
            }
            let parsed = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Stored>(&b).ok())
                .filter(|s| valid_namespace(&s.ns) && file_name(&s.ns) == name);
            let Some(s) = parsed else {
                tracing::warn!(file = %path.display(), "unreadable settings file; moved aside as .corrupt");
                let _ = std::fs::rename(&path, path.with_extension("corrupt"));
                continue;
            };
            let bytes = json_len(&s.value);
            docs.total += bytes;
            docs.map.insert(
                s.ns,
                Doc {
                    rev: s.rev,
                    updated_at: s.updated_at,
                    value: s.value,
                    bytes,
                },
            );
        }
        tracing::info!(dir = %dir.display(), docs = docs.map.len(), bytes = docs.total, "settings store opened");
        Ok(Self::build(Some(dir), docs))
    }

    /// Every applied write's `settings.changed` frame, in rev order. Subscribe BEFORE taking the
    /// snapshot, so no change can fall between the two (a change in both is harmless: clients
    /// ignore a rev they already have).
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.0.tx.subscribe()
    }

    /// `{"type":"settings.snapshot","docs":[{ns, rev, updatedAt, value}, …]}`: every namespace.
    pub async fn snapshot_frame(&self) -> String {
        let docs = self.0.docs.lock().await;
        let frame = SnapshotFrame {
            kind: "settings.snapshot",
            docs: docs
                .map
                .iter()
                .map(|(ns, d)| DocRef {
                    ns,
                    rev: d.rev,
                    updated_at: d.updated_at,
                    value: &d.value,
                })
                .collect(),
        };
        serde_json::to_string(&frame).expect("settings frames always serialize")
    }

    /// The current value and rev of one namespace (tests and debugging).
    pub async fn get(&self, ns: &str) -> Option<(u64, Value)> {
        let docs = self.0.docs.lock().await;
        docs.map.get(ns).map(|d| (d.rev, d.value.clone()))
    }

    /// Applies one write. On success the `settings.changed` frame has gone to every subscriber
    /// and `Ok(None)` is returned. An `if_absent` set on an existing namespace changes nothing
    /// and returns `Ok(Some(frame))`: the CURRENT document as a `settings.changed` with the
    /// writer's `opId` and the unchanged rev, for the writer alone (it adopts the shared value).
    pub async fn apply(
        &self,
        ns: &str,
        op_id: &str,
        op: SettingsOp,
    ) -> Result<Option<String>, SettingsError> {
        if !valid_namespace(ns) {
            return Err(SettingsError::BadNamespace);
        }
        if !valid_op_id(op_id) {
            return Err(SettingsError::BadOpId);
        }
        let mut docs = self.0.docs.lock().await;
        let old = docs.map.get(ns);
        let value = match op {
            SettingsOp::Set { value, if_absent } => {
                if if_absent && let Some(d) = old {
                    return Ok(Some(changed_frame(ns, d, op_id)));
                }
                value
            }
            SettingsOp::Patch { set, remove } => {
                if set
                    .keys()
                    .chain(remove.iter())
                    .any(|k| k.is_empty() || k.len() > MAX_KEY_LEN)
                {
                    return Err(SettingsError::BadKey);
                }
                let mut map = match old.map(|d| &d.value) {
                    None => Map::new(),
                    Some(Value::Object(m)) => m.clone(),
                    Some(_) => return Err(SettingsError::NotAMap),
                };
                for k in &remove {
                    map.remove(k);
                }
                map.extend(set);
                Value::Object(map)
            }
        };
        let bytes = json_len(&value);
        if bytes > MAX_DOC_BYTES {
            return Err(SettingsError::TooLarge);
        }
        let old_bytes = old.map_or(0, |d| d.bytes);
        if old.is_none() && docs.map.len() >= MAX_DOCS {
            return Err(SettingsError::StoreFull);
        }
        if docs.total - old_bytes + bytes > MAX_TOTAL_BYTES {
            return Err(SettingsError::StoreFull);
        }
        let doc = Doc {
            rev: old.map_or(0, |d| d.rev) + 1,
            updated_at: now_ms(),
            value,
            bytes,
        };

        // Durable first: only a write that is on disk is applied and announced.
        if let Some(dir) = self.0.dir.clone() {
            let stored = Stored {
                ns: ns.to_owned(),
                rev: doc.rev,
                updated_at: doc.updated_at,
                value: doc.value.clone(),
            };
            let bytes = serde_json::to_vec(&stored).expect("settings documents always serialize");
            let name = file_name(ns);
            let written = tokio::task::spawn_blocking(move || write_atomic(&dir, &name, &bytes))
                .await
                .unwrap_or_else(|e| Err(io::Error::other(e)));
            if let Err(e) = written {
                tracing::error!(ns, error = %e, "settings write failed; the change was not applied");
                return Err(SettingsError::Unavailable);
            }
        }

        let frame: Arc<str> = changed_frame(ns, &doc, op_id).into();
        docs.total = docs.total - old_bytes + doc.bytes;
        docs.map.insert(ns.to_owned(), doc);
        // No receivers (no connection open) is not an error: the write is stored.
        let _ = self.0.tx.send(frame);
        Ok(None)
    }
}

// Frames are structs, not json!(): serde_json's Map sorts keys, and the clients rely on every
// server frame starting with `{"type":` to route it cheaply (the feeds' tee in apps/*/feed).

#[derive(Serialize)]
struct DocRef<'a> {
    ns: &'a str,
    rev: u64,
    #[serde(rename = "updatedAt")]
    updated_at: u64,
    value: &'a Value,
}

#[derive(Serialize)]
struct SnapshotFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    docs: Vec<DocRef<'a>>,
}

#[derive(Serialize)]
struct ChangedFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    doc: DocRef<'a>,
    #[serde(rename = "opId")]
    op_id: &'a str,
}

fn changed_frame(ns: &str, d: &Doc, op_id: &str) -> String {
    let frame = ChangedFrame {
        kind: "settings.changed",
        doc: DocRef {
            ns,
            rev: d.rev,
            updated_at: d.updated_at,
            value: &d.value,
        },
        op_id,
    };
    serde_json::to_string(&frame).expect("settings frames always serialize")
}

/// `settings.rejected` for the writer (ws.rs). `ns`/`opId` are echoed as given (possibly empty
/// when the frame was too malformed to read them), so the client can drop the right edit.
pub fn rejected_frame(ns: &str, op_id: &str, code: &str, message: &str) -> String {
    #[derive(Serialize)]
    struct Rejected<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        ns: &'a str,
        #[serde(rename = "opId")]
        op_id: &'a str,
        code: &'a str,
        message: &'a str,
    }
    let frame = Rejected {
        kind: "settings.rejected",
        ns,
        op_id,
        code,
        message,
    };
    serde_json::to_string(&frame).expect("settings frames always serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(v: Value) -> SettingsOp {
        SettingsOp::Set {
            value: v,
            if_absent: false,
        }
    }

    fn patch(set: Value, remove: &[&str]) -> SettingsOp {
        SettingsOp::Patch {
            set: set.as_object().unwrap().clone(),
            remove: remove.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn namespaces_are_checked_and_file_names_are_case_safe() {
        for ok in [
            "layers",
            "drawings:SMPL",
            "a.b_c-d:e",
            &"x".repeat(MAX_NS_LEN),
        ] {
            assert!(valid_namespace(ok), "{ok}");
        }
        for bad in [
            "",
            ".hidden",
            "../x",
            "a/b",
            "a\\b",
            "a b",
            "é",
            &"x".repeat(MAX_NS_LEN + 1),
        ] {
            assert!(!valid_namespace(bad), "{bad}");
        }
        // Differ only in case: distinct files even where the file system ignores case.
        assert_ne!(
            file_name("drawings:abc").to_lowercase(),
            file_name("drawings:ABC").to_lowercase()
        );
        assert_eq!(file_name("ab"), "6162.json");
    }

    #[tokio::test]
    async fn last_writer_wins_in_arrival_order_and_revs_climb() {
        let s = Settings::in_memory();
        let mut rx = s.subscribe();
        s.apply("layers", "a1", set(json!({"halts": false})))
            .await
            .unwrap();
        s.apply("layers", "b1", set(json!({"halts": true})))
            .await
            .unwrap();
        assert_eq!(s.get("layers").await, Some((2, json!({"halts": true}))));
        let first: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        let second: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(
            (first["rev"].clone(), first["opId"].clone()),
            (json!(1), json!("a1"))
        );
        assert_eq!(
            (second["rev"].clone(), second["opId"].clone()),
            (json!(2), json!("b1"))
        );
        assert_eq!(second["type"], "settings.changed");
        // `type` first, as the clients' frame routing expects.
        assert!(
            s.snapshot_frame()
                .await
                .starts_with(r#"{"type":"settings.snapshot","#)
        );
        assert!(rejected_frame("a", "b", "c", "d").starts_with(r#"{"type":"settings.rejected","#));
        assert_eq!(second["value"], json!({"halts": true}));
    }

    #[tokio::test]
    async fn if_absent_seeds_once_then_answers_the_writer_with_the_shared_value() {
        let s = Settings::in_memory();
        let mut rx = s.subscribe();
        assert_eq!(
            s.apply(
                "indicators",
                "m1",
                SettingsOp::Set {
                    value: json!(["vwap"]),
                    if_absent: true
                }
            )
            .await
            .unwrap(),
            None
        );
        rx.recv().await.unwrap();
        let reply = s
            .apply(
                "indicators",
                "m2",
                SettingsOp::Set {
                    value: json!(["ema-9"]),
                    if_absent: true,
                },
            )
            .await
            .unwrap()
            .expect("a direct reply for the second migrator");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            (v["rev"].clone(), v["value"].clone(), v["opId"].clone()),
            (json!(1), json!(["vwap"]), json!("m2"))
        );
        // Nothing was broadcast for the refused seed.
        assert!(rx.try_recv().is_err());
        assert_eq!(s.get("indicators").await, Some((1, json!(["vwap"]))));
    }

    #[tokio::test]
    async fn patches_merge_entries_so_different_drawings_never_overwrite_each_other() {
        let s = Settings::in_memory();
        s.apply("drawings:SMPL", "a", patch(json!({"d1": {"z": 1}}), &[]))
            .await
            .unwrap();
        // Two devices, two drawings, back to back: both survive.
        s.apply("drawings:SMPL", "b", patch(json!({"d2": {"z": 2}}), &[]))
            .await
            .unwrap();
        s.apply(
            "drawings:SMPL",
            "c",
            patch(json!({"d1": {"z": 1, "moved": true}}), &[]),
        )
        .await
        .unwrap();
        assert_eq!(
            s.get("drawings:SMPL").await.unwrap().1,
            json!({"d1": {"z": 1, "moved": true}, "d2": {"z": 2}})
        );
        s.apply(
            "drawings:SMPL",
            "d",
            patch(json!({}), &["d1", "never-there"]),
        )
        .await
        .unwrap();
        assert_eq!(
            s.get("drawings:SMPL").await,
            Some((4, json!({"d2": {"z": 2}})))
        );
        // A patch needs an object document.
        s.apply("layers", "e", set(json!([1, 2]))).await.unwrap();
        assert_eq!(
            s.apply("layers", "f", patch(json!({"a": 1}), &[])).await,
            Err(SettingsError::NotAMap)
        );
        assert_eq!(
            s.apply("drawings:SMPL", "g", patch(json!({"": 1}), &[]))
                .await,
            Err(SettingsError::BadKey)
        );
    }

    #[tokio::test]
    async fn oversized_values_and_bad_names_are_refused_and_change_nothing() {
        let s = Settings::in_memory();
        let big = json!("x".repeat(MAX_DOC_BYTES));
        assert_eq!(
            s.apply("layers", "a", set(big)).await,
            Err(SettingsError::TooLarge)
        );
        assert_eq!(
            s.apply("../etc", "a", set(json!(1))).await,
            Err(SettingsError::BadNamespace)
        );
        assert_eq!(
            s.apply("layers", "", set(json!(1))).await,
            Err(SettingsError::BadOpId)
        );
        assert_eq!(
            s.apply("layers", &"o".repeat(65), set(json!(1))).await,
            Err(SettingsError::BadOpId)
        );
        assert_eq!(s.get("layers").await, None);

        // The total cap: fill with documents just under the per-document limit.
        let chunk = json!("y".repeat(MAX_DOC_BYTES - 16));
        let mut n = 0;
        let err = loop {
            match s
                .apply(&format!("drawings:S{n}"), "a", set(chunk.clone()))
                .await
            {
                Ok(_) => n += 1,
                Err(e) => break e,
            }
        };
        assert_eq!(err, SettingsError::StoreFull);
        assert_eq!(n, MAX_TOTAL_BYTES / (MAX_DOC_BYTES - 14));
        // Replacing an existing document with a smaller one is still allowed when full.
        s.apply("drawings:S0", "b", set(json!("small")))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn documents_survive_a_restart_and_stray_files_do_not_break_loading() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("settings");
        {
            let s = Settings::open(&root).unwrap();
            s.apply("chartSettings", "a", set(json!({"grid": false})))
                .await
                .unwrap();
            s.apply("chartSettings", "b", set(json!({"grid": true})))
                .await
                .unwrap();
            s.apply("drawings:SMPL", "c", patch(json!({"d1": {"z": 1}}), &[]))
                .await
                .unwrap();
        }
        // A crash mid-write leaves a temp file; a hand-edit can leave junk.
        std::fs::write(
            root.join(format!("{}.tmp", file_name("chartSettings"))),
            "{\"torn",
        )
        .unwrap();
        std::fs::write(root.join(file_name("broken")), "not json").unwrap();
        // A file whose name does not match its namespace is not trusted either.
        std::fs::write(
            root.join(file_name("other")),
            json!({"ns": "layers", "rev": 9, "updatedAt": 0, "value": 1}).to_string(),
        )
        .unwrap();

        let s = Settings::open(&root).unwrap();
        assert_eq!(
            s.get("chartSettings").await,
            Some((2, json!({"grid": true})))
        );
        assert_eq!(
            s.get("drawings:SMPL").await,
            Some((1, json!({"d1": {"z": 1}})))
        );
        assert_eq!(s.get("broken").await, None);
        assert_eq!(s.get("layers").await, None);
        assert!(
            !root
                .join(format!("{}.tmp", file_name("chartSettings")))
                .exists()
        );
        assert!(
            root.join(file_name("broken"))
                .with_extension("corrupt")
                .exists()
        );
        // Revs carry on from the stored ones.
        s.apply("chartSettings", "d", set(json!({}))).await.unwrap();
        assert_eq!(s.get("chartSettings").await.unwrap().0, 3);
        let snap: Value = serde_json::from_str(&s.snapshot_frame().await).unwrap();
        assert_eq!(snap["type"], "settings.snapshot");
        assert_eq!(snap["docs"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failed_disk_write_is_refused_and_not_applied() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("settings");
        let s = Settings::open(&root).unwrap();
        s.apply("layers", "a", set(json!(1))).await.unwrap();
        // The directory disappears under the server (volume gone): the write must not pretend.
        std::fs::remove_dir_all(&root).unwrap();
        let mut rx = s.subscribe();
        assert_eq!(
            s.apply("layers", "b", set(json!(2))).await,
            Err(SettingsError::Unavailable)
        );
        assert_eq!(s.get("layers").await, Some((1, json!(1))));
        assert!(rx.try_recv().is_err());
    }
}
