//! The SVN dumpstream reader (RFC 006 D-1, Tier D) — the one genuinely new parser.
//!
//! A dumpstream (`svnadmin dump` output) is uncompressed framed plaintext: a format-version line, a UUID,
//! then a sequence of **Revision** and **Node** records, each a block of `Header: value` lines followed by
//! a length-prefixed body (`Content-length` = `Prop-content-length` + `Text-content-length`). Content is
//! read by **exact declared length**, never by scanning (text is arbitrary bytes). Every declared length
//! is bounds-checked against the bytes actually present — a malformed or hostile dump is a typed refusal,
//! never a panic or an over-allocation (RFC 006 security review, `brygge-03` T-2/T-8/INV-2).
//!
//! Supported: dump format versions 1–3, **fulltext and delta** (RFC 013 D-2): `svnadmin dump`, `svnadmin dump
//! --deltas` and `svnrdump dump`. A `Text-delta: true` body is svndiff and a `Prop-delta: true` block may delete
//! properties; both are kept as the dump states them (a [`TextBody::Delta`], a [`PropBlock::Delta`]) and applied
//! to the node's base by the tree, which is the one place that knows the base. **Refused:** an unknown format
//! version, and svndiff version 1 or 2 (compressed; named, OQ-4).

use crate::source::Limits;
use crate::{Error, svndiff};

/// A property list in dump order (`svn:*` and custom). Keys are UTF-8; values are arbitrary bytes.
pub type Props = Vec<(String, Vec<u8>)>;

/// One entry of a property **delta**: a property set to a value, or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropChange {
    /// `K <len>` / `V <len>`: set the property.
    Set(String, Vec<u8>),
    /// `D <len>`: remove the property.
    Delete(String),
}

/// A node's property block, as the dump states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropBlock {
    /// A **full** block (no `Prop-delta`): the node's complete property set (a full replacement), possibly
    /// empty (properties cleared).
    Full(Props),
    /// A **delta** block (`Prop-delta: true`): changes against the node's base properties, in order.
    Delta(Vec<PropChange>),
}

/// A node's text, as the dump states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextBody {
    /// The fulltext (`Text-delta` absent or not `true`).
    Full(Vec<u8>),
    /// svndiff version 0 (`Text-delta: true`), to be applied to the node's base text. Its header has been
    /// checked; the windows have not been read.
    Delta(Vec<u8>),
}

/// The checksum headers a node states (lowercase hex as written). Each is a **consistency** check the tree
/// makes against the text it reconstructs, never authenticity: a dump is untrusted and states any checksum
/// it likes. `svnadmin` writes MD5 and SHA-1, `svnrdump` MD5 only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checksums {
    /// `Text-content-md5`: the resulting text.
    pub content_md5: Option<String>,
    /// `Text-content-sha1`.
    pub content_sha1: Option<String>,
    /// `Text-delta-base-md5`: the base a delta applies to (written when the base is non-empty).
    pub base_md5: Option<String>,
    /// `Text-delta-base-sha1`.
    pub base_sha1: Option<String>,
    /// `Text-copy-source-md5`: the copy source's text, on a copy with text.
    pub copy_md5: Option<String>,
    /// `Text-copy-source-sha1`.
    pub copy_sha1: Option<String>,
}
/// A node's kind. Absent on a `delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A file.
    File,
    /// A directory.
    Dir,
}

/// What a node record does to its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeAction {
    /// Create the path.
    Add,
    /// Remove the path (and, for a directory, its subtree).
    Delete,
    /// Change the path's content/properties.
    Change,
    /// Remove the node at the path and add a new one there in the same revision.
    Replace,
}

/// One node record within a revision.
#[derive(Debug, Clone)]
pub struct NodeRecord {
    /// The repo-relative path this node concerns.
    pub path: String,
    /// The node kind (`None` on a delete).
    pub kind: Option<NodeKind>,
    /// The action.
    pub action: NodeAction,
    /// The source of a copy, if any: `(copyfrom-rev, copyfrom-path)`. A source-stated copy (SRC-S3).
    pub copyfrom: Option<(u64, String)>,
    /// The node's property block. `None` = the node carried **no** property block (properties unchanged from
    /// its base); `Some(Full(_))` = the node's **complete** property set (a full replacement), possibly empty
    /// (properties cleared); `Some(Delta(_))` = changes against the base's properties (`Prop-delta: true`).
    pub props: Option<PropBlock>,
    /// The node's text, if it carried any: fulltext, or svndiff against the base.
    pub text: Option<TextBody>,
    /// The checksum headers the node stated.
    pub checksums: Checksums,
}

/// One revision record and its nodes.
#[derive(Debug, Clone)]
pub struct RevisionRecord {
    /// The global, monotonically increasing revision number.
    pub number: u64,
    /// The revision properties (`svn:author`, `svn:date`, `svn:log`, and any custom).
    pub props: Props,
    /// The node records within this revision, in dump order.
    pub nodes: Vec<NodeRecord>,
}

/// A parsed dumpstream.
#[derive(Debug, Clone)]
pub struct Dump {
    /// The repository UUID, if the dump declared one.
    pub uuid: Option<String>,
    /// The revisions, in order.
    pub revisions: Vec<RevisionRecord>,
}

/// A bounds-checked cursor over the dumpstream bytes. Never indexes or slices without a checked `get`.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(b'\n') {
            self.pos = self.pos.saturating_add(1);
        }
    }

    /// Read up to and past the next `\n`, returning the line without the newline. `None` at EOF.
    fn read_line(&mut self) -> Option<&'a [u8]> {
        if self.eof() {
            return None;
        }
        let rest = self.data.get(self.pos..)?;
        match rest.iter().position(|&b| b == b'\n') {
            Some(i) => {
                let line = rest.get(..i)?;
                self.pos = self.pos.saturating_add(i).saturating_add(1);
                Some(line)
            }
            None => {
                self.pos = self.data.len();
                Some(rest)
            }
        }
    }

    /// Read exactly `n` bytes, bounds-checked (a declared length beyond the input is a refusal).
    fn read_exact(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| Error::Read("length overflow".to_string()))?;
        let slice = self.data.get(self.pos..end).ok_or_else(|| {
            Error::Read("a declared content length runs past the end of the dumpstream".to_string())
        })?;
        self.pos = end;
        Ok(slice)
    }

    fn consume_newline(&mut self) -> Result<(), Error> {
        match self.peek() {
            Some(b'\n') => {
                self.pos = self.pos.saturating_add(1);
                Ok(())
            }
            _ => Err(Error::Read(
                "expected a newline delimiter in the dumpstream".to_string(),
            )),
        }
    }
}

fn header_get<'a>(headers: &'a [(String, String)], key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn header_usize(headers: &[(String, String)], key: &str) -> Result<Option<usize>, Error> {
    match header_get(headers, key) {
        None => Ok(None),
        Some(v) => v
            .parse::<usize>()
            .map(Some)
            .map_err(|_| Error::Read(format!("header {key} is not a number: {v}"))),
    }
}

fn header_true(headers: &[(String, String)], key: &str) -> bool {
    header_get(headers, key) == Some("true")
}

/// How many bytes of an offending line an error message shows, so a hostile dump cannot make a message
/// arbitrarily long.
const MAX_SHOWN_BYTES: usize = 256;

/// Render `bytes` as valid UTF-8 kept verbatim and each invalid byte escaped as `\xNN` — never a lossy
/// substitution, which would silently alter the bytes an error message names.
pub(crate) fn escape_invalid_utf8(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                out.push_str(
                    std::str::from_utf8(rest.get(..valid_up_to).unwrap_or(&[])).unwrap_or_default(),
                );
                let bad_len = e.error_len().unwrap_or(rest.len() - valid_up_to).max(1);
                let bad_end = (valid_up_to + bad_len).min(rest.len());
                for &b in rest.get(valid_up_to..bad_end).unwrap_or(&[]) {
                    let _ = write!(out, "\\x{b:02X}");
                }
                rest = rest.get(bad_end..).unwrap_or(&[]);
            }
        }
    }
    out
}

/// Read a header block: `Header: value` lines until a blank line or EOF.
fn read_header_block(cur: &mut Cursor) -> Result<Vec<(String, String)>, Error> {
    let mut out = Vec::new();
    while let Some(line) = cur.read_line() {
        if line.is_empty() {
            break;
        }
        let s = std::str::from_utf8(line).map_err(|_| {
            Error::Read(format!(
                "a dumpstream header line (a node path, for instance) is not valid UTF-8 (invalid bytes \
                 shown as \\xNN): {}",
                escape_invalid_utf8(line.get(..MAX_SHOWN_BYTES).unwrap_or(line))
            ))
        })?;
        match s.split_once(": ") {
            Some((k, v)) => out.push((k.to_string(), v.to_string())),
            None => {
                return Err(Error::Read(format!(
                    "malformed dumpstream header line: {s}"
                )));
            }
        }
    }
    Ok(out)
}

/// A record's body, split.
struct Body {
    props: Option<PropBlock>,
    text: Option<TextBody>,
}

/// Read a record body per its `Content-length`, split into (properties, text). The property result is `None`
/// when the record carried no property block (`Prop-content-length` absent), distinct from an empty block.
/// `Prop-delta: true` makes the block a delta (and only then may it hold `D` entries); `Text-delta: true` makes
/// the text an svndiff, whose header is checked here (so an unsupported version is refused before any tree is
/// built). A fulltext over `limits.max_node_text_bytes` is `ResourceLimit`. `node_path` names a node in errors.
fn read_body(
    cur: &mut Cursor,
    headers: &[(String, String)],
    node_path: Option<&str>,
    limits: &Limits,
) -> Result<Body, Error> {
    let Some(content_len) = header_usize(headers, "Content-length")? else {
        return Ok(Body {
            props: None,
            text: None,
        });
    };
    let prop_len_opt = header_usize(headers, "Prop-content-length")?;
    let prop_len = prop_len_opt.unwrap_or(0);
    let text_len = header_usize(headers, "Text-content-length")?;

    // Consistency: the content is exactly the property block plus the text.
    let expected = prop_len
        .checked_add(text_len.unwrap_or(0))
        .ok_or_else(|| Error::Read("length overflow".to_string()))?;
    if expected != content_len {
        return Err(Error::Read(format!(
            "Content-length {content_len} does not equal Prop-content-length {prop_len} + \
             Text-content-length {}",
            text_len.unwrap_or(0)
        )));
    }
    let prop_delta = header_true(headers, "Prop-delta");
    let text_delta = header_true(headers, "Text-delta");
    let at = node_path.map_or_else(String::new, |p| format!("{p}: "));

    // The ceiling on one node's text, before the bytes are copied out (a delta's own bytes are bounded by the
    // dump; what it expands to is checked when it is applied).
    if let Some(tl) = text_len {
        if !text_delta && tl > limits.max_node_text_bytes {
            return Err(over_limit("a node's text", limits.max_node_text_bytes));
        }
    }

    let body = cur.read_exact(content_len)?;
    let props = match prop_len_opt {
        Some(_) => {
            let block = body
                .get(..prop_len)
                .ok_or_else(|| Error::Read("property block exceeds body".to_string()))?;
            Some(parse_props(block, prop_delta)?)
        }
        None => None,
    };
    let text = match text_len {
        Some(_) => {
            let block = body
                .get(prop_len..content_len)
                .ok_or_else(|| Error::Read("text block exceeds body".to_string()))?;
            if text_delta {
                check_svndiff_header(block, &at)?;
                Some(TextBody::Delta(block.to_vec()))
            } else {
                Some(TextBody::Full(block.to_vec()))
            }
        }
        None => None,
    };
    Ok(Body { props, text })
}

/// Refuse svndiff version 1 or 2 by name, and anything that is not an svndiff header.
fn check_svndiff_header(block: &[u8], at: &str) -> Result<(), Error> {
    match svndiff::check_header(block) {
        Ok(()) => Ok(()),
        Err(svndiff::DiffError::UnsupportedVersion(v)) => {
            let which = if v == 1 { "1 (zlib)" } else { "2 (lz4)" };
            Err(Error::UnsupportedFormat {
                what: format!("svndiff version {which}"),
                reason: format!(
                    "{at}this build reads uncompressed svndiff (version 0); re-dump with `svnadmin dump` \
                     (fulltext or `--deltas`) or with `svnrdump`, which write version 0"
                ),
            })
        }
        Err(svndiff::DiffError::Malformed(m)) => Err(Error::Read(format!(
            "{at}the text delta is not svndiff: {m}"
        ))),
        Err(svndiff::DiffError::TooLarge { .. }) => {
            Err(Error::Read(format!("{at}the text delta is not svndiff")))
        }
    }
}

fn over_limit(what: &str, max_bytes: usize) -> Error {
    Error::ResourceLimit {
        what: what.to_string(),
        ceiling: format!("{max_bytes} bytes"),
    }
}

/// Parse a property block: repeated `K <len>\n<key>\nV <len>\n<value>\n` (and, in a **delta** block only,
/// `D <len>\n<key>\n`), terminated by `PROPS-END`.
fn parse_props(block: &[u8], delta: bool) -> Result<PropBlock, Error> {
    let mut cur = Cursor::new(block);
    let mut full: Props = Vec::new();
    let mut changes: Vec<PropChange> = Vec::new();
    loop {
        let line = cur
            .read_line()
            .ok_or_else(|| Error::Read("unterminated property block (no PROPS-END)".to_string()))?;
        if line == b"PROPS-END" {
            break;
        }
        let s = std::str::from_utf8(line)
            .map_err(|_| Error::Read("property control line is not valid UTF-8".to_string()))?;
        if let Some(rest) = s.strip_prefix("K ") {
            let klen = rest
                .parse::<usize>()
                .map_err(|_| Error::Read(format!("bad property key length: {rest}")))?;
            let key = cur.read_exact(klen)?;
            cur.consume_newline()?;
            let vline = cur
                .read_line()
                .ok_or_else(|| Error::Read("property key without a value".to_string()))?;
            let vs = std::str::from_utf8(vline)
                .map_err(|_| Error::Read("property value control line is not UTF-8".to_string()))?;
            let vrest = vs
                .strip_prefix("V ")
                .ok_or_else(|| Error::Read(format!("expected 'V <len>' after a key, got: {vs}")))?;
            let vlen = vrest
                .parse::<usize>()
                .map_err(|_| Error::Read(format!("bad property value length: {vrest}")))?;
            let value = cur.read_exact(vlen)?.to_vec();
            cur.consume_newline()?;
            let key_str = std::str::from_utf8(key)
                .map_err(|_| Error::Read("property key is not valid UTF-8".to_string()))?
                .to_string();
            if delta {
                changes.push(PropChange::Set(key_str, value));
            } else {
                full.push((key_str, value));
            }
        } else if let Some(rest) = s.strip_prefix("D ") {
            // A property deletion appears only in a `Prop-delta` block.
            if !delta {
                return Err(Error::Read(
                    "a property deletion (`D`) in a property block that is not a delta".to_string(),
                ));
            }
            let klen = rest
                .parse::<usize>()
                .map_err(|_| Error::Read(format!("bad property key length: {rest}")))?;
            let key = cur.read_exact(klen)?;
            cur.consume_newline()?;
            let key_str = std::str::from_utf8(key)
                .map_err(|_| Error::Read("property key is not valid UTF-8".to_string()))?
                .to_string();
            changes.push(PropChange::Delete(key_str));
        } else {
            return Err(Error::Read(format!("malformed property control line: {s}")));
        }
    }
    Ok(if delta {
        PropBlock::Delta(changes)
    } else {
        PropBlock::Full(full)
    })
}

fn parse_node(
    cur: &mut Cursor,
    headers: &[(String, String)],
    limits: &Limits,
) -> Result<NodeRecord, Error> {
    let path = header_get(headers, "Node-path")
        .ok_or_else(|| Error::Read("node record without Node-path".to_string()))?
        .to_string();
    let action = match header_get(headers, "Node-action") {
        Some("add") => NodeAction::Add,
        Some("delete") => NodeAction::Delete,
        Some("change") => NodeAction::Change,
        Some("replace") => NodeAction::Replace,
        other => {
            return Err(Error::Read(format!(
                "unknown or missing Node-action: {}",
                other.unwrap_or("(absent)")
            )));
        }
    };
    let kind = match header_get(headers, "Node-kind") {
        Some("file") => Some(NodeKind::File),
        Some("dir") => Some(NodeKind::Dir),
        None => None,
        Some(other) => return Err(Error::Read(format!("unknown Node-kind: {other}"))),
    };
    let copyfrom = match (
        header_get(headers, "Node-copyfrom-rev"),
        header_get(headers, "Node-copyfrom-path"),
    ) {
        (Some(r), Some(p)) => {
            let rev = r
                .parse::<u64>()
                .map_err(|_| Error::Read(format!("bad Node-copyfrom-rev: {r}")))?;
            Some((rev, p.to_string()))
        }
        _ => None,
    };
    let Body { props, text } = read_body(cur, headers, Some(&path), limits)?;
    let hex = |key: &str| header_get(headers, key).map(str::to_string);
    let checksums = Checksums {
        content_md5: hex("Text-content-md5"),
        content_sha1: hex("Text-content-sha1"),
        base_md5: hex("Text-delta-base-md5"),
        base_sha1: hex("Text-delta-base-sha1"),
        copy_md5: hex("Text-copy-source-md5"),
        copy_sha1: hex("Text-copy-source-sha1"),
    };
    Ok(NodeRecord {
        path,
        kind,
        action,
        copyfrom,
        props,
        text,
        checksums,
    })
}

/// Parse a whole dumpstream into a [`Dump`], with the default ceilings.
///
/// # Errors
/// As [`parse_dump_with`].
#[cfg(test)]
pub fn parse_dump(data: &[u8]) -> Result<Dump, Error> {
    parse_dump_with(data, &Limits::default())
}

/// Parse a whole dumpstream into a [`Dump`].
///
/// # Errors
/// [`Error::Read`] on malformed framing; [`Error::UnsupportedFormat`] for an unknown format version or an
/// svndiff version 1 or 2; [`Error::ResourceLimit`] for a fulltext node over `limits.max_node_text_bytes`.
pub fn parse_dump_with(data: &[u8], limits: &Limits) -> Result<Dump, Error> {
    let mut cur = Cursor::new(data);
    let mut format_version: Option<u32> = None;
    let mut uuid: Option<String> = None;
    let mut revisions: Vec<RevisionRecord> = Vec::new();

    loop {
        cur.skip_newlines();
        if cur.eof() {
            break;
        }
        let headers = read_header_block(&mut cur)?;
        if headers.is_empty() {
            continue;
        }
        if let Some(v) = header_get(&headers, "SVN-fs-dump-format-version") {
            let ver = v
                .parse::<u32>()
                .map_err(|_| Error::Read(format!("bad dump format version: {v}")))?;
            if !(1..=3).contains(&ver) {
                return Err(Error::UnsupportedFormat {
                    what: format!("dump format version {ver}"),
                    reason: "this build reads dump format versions 1–3".to_string(),
                });
            }
            format_version = Some(ver);
            continue;
        }
        if let Some(u) = header_get(&headers, "UUID") {
            uuid = Some(u.to_string());
            continue;
        }
        if let Some(n) = header_get(&headers, "Revision-number") {
            let number = n
                .parse::<u64>()
                .map_err(|_| Error::Read(format!("bad Revision-number: {n}")))?;
            let body = read_body(&mut cur, &headers, None, limits)?;
            let props = match body.props {
                Some(PropBlock::Full(p)) => p,
                // A revision's properties are never a delta; one that says so is read as its entries set.
                Some(PropBlock::Delta(ch)) => ch
                    .into_iter()
                    .filter_map(|c| match c {
                        PropChange::Set(k, v) => Some((k, v)),
                        PropChange::Delete(_) => None,
                    })
                    .collect(),
                None => Vec::new(),
            };
            revisions.push(RevisionRecord {
                number,
                props,
                nodes: Vec::new(),
            });
            continue;
        }
        if header_get(&headers, "Node-path").is_some() {
            let node = parse_node(&mut cur, &headers, limits)?;
            revisions
                .last_mut()
                .ok_or_else(|| {
                    Error::Read("a node record appeared before any revision".to_string())
                })?
                .nodes
                .push(node);
            continue;
        }
        // An unrecognized record: consume any body it declares, then move on.
        read_body(&mut cur, &headers, None, limits)?;
    }

    // The version is validated (range-checked) as it's parsed above; its only remaining obligation is
    // to have been present at all (a dump with no version header is malformed).
    format_version
        .ok_or_else(|| Error::Read("missing SVN-fs-dump-format-version header".to_string()))?;
    Ok(Dump { uuid, revisions })
}

#[cfg(test)]
mod tests;
