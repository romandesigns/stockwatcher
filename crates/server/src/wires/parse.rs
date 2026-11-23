//! RSS 2.0 and Atom parsing, on `quick-xml` (already in Cargo.lock; MIT, maintained, no new
//! transitive dependency). Only the few fields the wires module reads are kept: id, title, link,
//! summary and the published time.
//!
//! Times: RSS `pubDate` is RFC 2822 (`+0000`, `GMT`, `UT`, `EST`/`EDT` all parse); Atom
//! `published`/`updated` and `dc:date` are RFC 3339. A time **without a zone** is not guessed
//! (`published_at_ns = None`): a wrong guess by the four or five hours between UTC and ET would
//! make a stale release look fresh or a fresh one stale, and no zone is safer to say than a wrong
//! one. The raw string is kept either way.

use chrono::DateTime;
use quick_xml::Reader;
use quick_xml::events::Event;

/// One feed entry as parsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// `guid` (RSS) / `id` (Atom); empty when the feed gave none.
    pub guid: String,
    pub title: String,
    pub link: String,
    /// The teaser as text (HTML tags removed, entities decoded, whitespace collapsed).
    pub summary: String,
    /// The published time exactly as delivered.
    pub published_raw: Option<String>,
    /// Parsed (UTC ns); `None` when missing, unparseable or zone-less (module docs).
    pub published_at_ns: Option<i64>,
}

impl Entry {
    /// The identity used to recognise the same entry on the next poll: the guid, else the link,
    /// else the title.
    pub fn key(&self) -> &str {
        [&self.guid, &self.link, &self.title]
            .into_iter()
            .find(|s| !s.is_empty())
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// Parses an RSS 2.0 or Atom document. A malformed document is an error as a whole (no partial
/// list): entries parsed before the fault cannot be told from a truncated response.
pub fn parse_feed(xml: &str) -> Result<Vec<Entry>, String> {
    let mut r = Reader::from_str(xml);
    r.config_mut().trim_text(false);
    let mut out = Vec::new();
    let mut cur: Option<Entry> = None;
    // The element whose text is being collected, inside an entry.
    let mut field: Option<&'static str> = None;
    let mut text = String::new();
    let mut depth_in_entry = 0usize;
    let mut saw_root = false;
    loop {
        let ev = r
            .read_event()
            .map_err(|e| format!("xml at {}: {e}", r.buffer_position()))?;
        match ev {
            Event::Start(e) => {
                let name = e.local_name();
                let name: &str = name.as_ref();
                if !saw_root {
                    if !matches!(name, "rss" | "feed" | "RDF") {
                        return Err(format!("not a feed: root <{name}>"));
                    }
                    saw_root = true;
                }
                if cur.is_none() {
                    if name == "item" || name == "entry" {
                        cur = Some(Entry::default());
                        depth_in_entry = 0;
                    }
                    continue;
                }
                depth_in_entry += 1;
                if depth_in_entry == 1 {
                    field = match name {
                        "guid" | "id" => Some("guid"),
                        "title" => Some("title"),
                        "link" => Some("link"),
                        "description" | "summary" => Some("summary"),
                        "pubDate" | "published" | "date" => Some("published"),
                        "updated" => Some("updated"),
                        _ => None,
                    };
                    text.clear();
                    // Atom: `<link href="..." rel="alternate">` usually comes as Empty, but a
                    // Start with an href is legal too.
                    if name == "link"
                        && let Some(href) = attr(&e, "href")
                        && let Some(c) = cur.as_mut()
                        && c.link.is_empty()
                    {
                        c.link = href;
                    }
                }
            }
            Event::Empty(e) => {
                let name = e.local_name();
                if cur.is_some() && depth_in_entry == 0 && AsRef::<str>::as_ref(&name) == "link" {
                    let rel = attr(&e, "rel");
                    if let (Some(href), Some(c)) = (attr(&e, "href"), cur.as_mut())
                        && (rel.is_none() || rel.as_deref() == Some("alternate"))
                        && c.link.is_empty()
                    {
                        c.link = href;
                    }
                }
                if !saw_root {
                    return Err("not a feed: empty root".into());
                }
            }
            Event::Text(t) => {
                if field.is_some() {
                    text.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if field.is_some() {
                    text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(g) => {
                if field.is_some() {
                    match g.resolve_char_ref() {
                        Ok(Some(c)) => text.push(c),
                        _ => {
                            if let Some(s) = quick_xml::escape::resolve_xml_entity(&g) {
                                text.push_str(s);
                            }
                        }
                    }
                }
            }
            Event::End(e) => {
                let Some(c) = cur.as_mut() else { continue };
                if depth_in_entry == 0 {
                    let name = e.local_name();
                    let name: &str = name.as_ref();
                    if (name == "item" || name == "entry")
                        && let Some(done) = cur.take()
                    {
                        out.push(done);
                    }
                    continue;
                }
                if depth_in_entry == 1
                    && let Some(f) = field.take()
                {
                    let v = text.trim().to_owned();
                    match f {
                        "guid" => c.guid = v,
                        "title" => c.title = squash(&strip_html(&v)),
                        "link" if c.link.is_empty() => c.link = v,
                        "summary" => c.summary = squash(&strip_html(&v)),
                        // Atom `published` wins over `updated` (an edit is not a new release).
                        "published" => c.published_raw = Some(v),
                        "updated" if c.published_raw.is_none() => c.published_raw = Some(v),
                        _ => {}
                    }
                }
                depth_in_entry -= 1;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_root {
        return Err("not a feed: no root element".into());
    }
    if cur.is_some() {
        return Err("truncated feed: unterminated entry".into());
    }
    for e in &mut out {
        e.published_at_ns = e.published_raw.as_deref().and_then(parse_time);
    }
    Ok(out)
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (AsRef::<str>::as_ref(&a.key.local_name()) == key).then(|| {
            quick_xml::escape::unescape(&a.value)
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| a.value.to_string())
        })
    })
}

/// RFC 2822 or RFC 3339 with a zone, as UTC ns. Zone-less forms are `None` (module docs).
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    DateTime::parse_from_rfc2822(s)
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .ok()
        .and_then(|t| t.timestamp_nanos_opt())
}

/// Removes HTML tags and decodes the handful of entities press-release teasers use.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let mut decoded = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(i) = rest.find('&') {
        decoded.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail[..tail.len().min(10)].find(';');
        let ent = end.map(|e| &tail[1..e]);
        let rep: Option<String> = ent.and_then(|e| match e {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" | "#39" => Some("'".into()),
            "nbsp" | "#160" => Some(" ".into()),
            _ => e
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32)
                .map(String::from),
        });
        match (rep, end) {
            (Some(r), Some(e)) => {
                decoded.push_str(&r);
                rest = &tail[e + 1..];
            }
            _ => {
                decoded.push('&');
                rest = &tail[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// Collapses runs of whitespace to one space and trims.
pub fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
