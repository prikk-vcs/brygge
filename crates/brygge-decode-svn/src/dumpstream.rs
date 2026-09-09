//! The SVN dumpstream reader (RFC 006 D-1, Tier D) — the one genuinely new parser.
//!
//! A dumpstream (`svnadmin dump` output) is uncompressed framed plaintext: a format-version line, a UUID,
//! then a sequence of **Revision** and **Node** records, each a block of `Header: value` lines followed by
//! a length-prefixed body (`Content-length` = `Prop-content-length` + `Text-content-length`). Content is
//! read by **exact declared length**, never by scanning (text is arbitrary bytes). Every declared length
//! is bounds-checked against the bytes actually present — a malformed or hostile dump is a typed refusal,
//! never a panic or an over-allocation (RFC 006 security review, `brygge-03` T-2/T-8/INV-2).
//!
//! Supported: fulltext dump format versions 1–3. **Refused:** an unknown version, and any **delta** node
//! (`Text-delta`/`Prop-delta: true` — svndiff is out of scope this increment, RFC 006 §4).

use crate::Error;

/// A property list in dump order (`svn:*` and custom). Keys are UTF-8; values are arbitrary bytes.
pub type Props = Vec<(String, Vec<u8>)>;

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
    /// The node's properties. `None` = the node carried **no** property block (properties unchanged from
    /// before); `Some(_)` = a property block was present and is the node's **complete** property set (a
    /// full replacement — SVN non-delta dump semantics), possibly empty (properties cleared).
    pub props: Option<Props>,
    /// The node's fulltext content, if it carried any.
    pub text: Option<Vec<u8>>,
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
    /// The dump format version (1–3).
    pub format_version: u32,
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

/// Read a header block: `Header: value` lines until a blank line or EOF.
fn read_header_block(cur: &mut Cursor) -> Result<Vec<(String, String)>, Error> {
    let mut out = Vec::new();
    while let Some(line) = cur.read_line() {
        if line.is_empty() {
            break;
        }
        let s = std::str::from_utf8(line)
            .map_err(|_| Error::Read("a dumpstream header line is not valid UTF-8".to_string()))?;
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

/// Read a record body per its `Content-length`, split into (properties, fulltext). The property result is
/// `None` when the record carried no property block (`Prop-content-length` absent), distinct from an
/// empty block (`Some(vec![])`).
fn read_body(
    cur: &mut Cursor,
    headers: &[(String, String)],
) -> Result<(Option<Props>, Option<Vec<u8>>), Error> {
    let Some(content_len) = header_usize(headers, "Content-length")? else {
        return Ok((None, None));
    };
    let prop_len_opt = header_usize(headers, "Prop-content-length")?;
    let prop_len = prop_len_opt.unwrap_or(0);
    let text_len = header_usize(headers, "Text-content-length")?;

    // Consistency: the content is exactly the property block plus the fulltext.
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

    let body = cur.read_exact(content_len)?;
    let props = match prop_len_opt {
        Some(_) => {
            let block = body
                .get(..prop_len)
                .ok_or_else(|| Error::Read("property block exceeds body".to_string()))?;
            Some(parse_props(block)?)
        }
        None => None,
    };
    let text = match text_len {
        Some(_) => {
            let block = body
                .get(prop_len..content_len)
                .ok_or_else(|| Error::Read("text block exceeds body".to_string()))?;
            Some(block.to_vec())
        }
        None => None,
    };
    Ok((props, text))
}

/// Parse a property block: repeated `K <len>\n<key>\nV <len>\n<value>\n`, terminated by `PROPS-END`.
fn parse_props(block: &[u8]) -> Result<Props, Error> {
    let mut cur = Cursor::new(block);
    let mut out = Vec::new();
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
            out.push((key_str, value));
        } else if s.starts_with("D ") {
            // A property deletion appears only in a `Prop-delta` block, which we refuse (RFC 006 §4).
            return Err(Error::UnsupportedFormat {
                what: "property-delta record".to_string(),
                reason: "delta dumps are out of scope this increment; re-dump without `--deltas`"
                    .to_string(),
            });
        } else {
            return Err(Error::Read(format!("malformed property control line: {s}")));
        }
    }
    Ok(out)
}

fn parse_node(cur: &mut Cursor, headers: &[(String, String)]) -> Result<NodeRecord, Error> {
    // Delta nodes are refused before any body is read (RFC 006 §4).
    if header_true(headers, "Text-delta") || header_true(headers, "Prop-delta") {
        return Err(Error::UnsupportedFormat {
            what: "delta dump".to_string(),
            reason: "svndiff deltas are out of scope this increment; a fulltext `svnadmin dump` \
                     (no `--deltas`) is required"
                .to_string(),
        });
    }
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
    let (props, text) = read_body(cur, headers)?;
    Ok(NodeRecord {
        path,
        kind,
        action,
        copyfrom,
        props,
        text,
    })
}

/// Parse a whole dumpstream into a [`Dump`].
///
/// # Errors
/// [`Error::Read`] on malformed framing; [`Error::UnsupportedFormat`] for an unknown format version or a
/// delta dump.
pub fn parse_dump(data: &[u8]) -> Result<Dump, Error> {
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
                    reason: "this build reads fulltext dump format versions 1–3".to_string(),
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
            let (props, _text) = read_body(&mut cur, &headers)?;
            revisions.push(RevisionRecord {
                number,
                props: props.unwrap_or_default(),
                nodes: Vec::new(),
            });
            continue;
        }
        if header_get(&headers, "Node-path").is_some() {
            let node = parse_node(&mut cur, &headers)?;
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
        read_body(&mut cur, &headers)?;
    }

    let format_version = format_version
        .ok_or_else(|| Error::Read("missing SVN-fs-dump-format-version header".to_string()))?;
    Ok(Dump {
        format_version,
        uuid,
        revisions,
    })
}

#[cfg(test)]
mod tests;
