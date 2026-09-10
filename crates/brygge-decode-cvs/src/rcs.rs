//! The RCS `,v` reader (RFC 007 D-1/§5a) — the one genuinely new parser.
//!
//! An RCS file is uncompressed text: an **admin** section (`head`, `symbols`, `expand`, …), a **delta**
//! section (per revision: `date`/`author`/`state`/`branches`/`next`), `desc`, and a **deltatext** section
//! (per revision: `log` and `text`). Strings are `@`-delimited with `@@` escaping a literal `@`. The `head`
//! revision's `text` is full content; other trunk revisions store **reverse** diffs down the `next` chain,
//! branch revisions store **forward** diffs — so any revision's content is reconstructed by walking the
//! delta path from `head`. Bounds-checked and panic-free on malformed input (untrusted, T-2/INV-2).

use std::collections::BTreeMap;

use crate::Error;

/// Maximum RCS file size the reader will accept (T-8). Correctness first; streaming a larger repository is
/// OQ-F (deferred).
pub const MAX_RCS_BYTES: usize = 512 * 1024 * 1024;
/// Maximum delta-chain length walked to reconstruct one revision (a malformed `,v` must not loop).
const MAX_CHAIN: usize = 1_000_000;

/// An RCS revision number, e.g. `1.3` (trunk) or `1.2.2.1` (a branch revision).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevNum(pub Vec<u32>);

impl RevNum {
    fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for p in s.split('.') {
            parts.push(p.parse::<u32>().ok()?);
        }
        Some(Self(parts))
    }

    /// Whether this is a trunk revision (`1.3` — two components).
    #[must_use]
    pub fn is_trunk(&self) -> bool {
        self.0.len() == 2
    }

    /// A dotted rendering (`1.2.2.1`).
    #[must_use]
    pub fn to_dotted(&self) -> String {
        self.0
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// One revision's metadata and stored text (full content for `head`, a diff otherwise).
#[derive(Debug, Clone)]
pub struct Revision {
    /// The revision number.
    pub num: RevNum,
    /// Commit time as epoch seconds (from the RCS `date`).
    pub date: i64,
    /// The committer login (a claim, PR-3).
    pub author: String,
    /// The RCS state (`Exp`, or `dead` for a deletion).
    pub state: String,
    /// Branch head revisions branching off this revision.
    pub branches: Vec<RevNum>,
    /// The next revision in the delta chain (older on the trunk, newer on a branch).
    pub next: Option<RevNum>,
    /// The log message (a claim, PR-3).
    pub log: Vec<u8>,
    /// The stored `text`: full content for `head`, an ed-style RCS diff otherwise.
    pub text: Vec<u8>,
}

/// A parsed RCS `,v` file.
#[derive(Debug, Clone)]
pub struct RcsFile {
    /// The head revision (its `text` is full content).
    pub head: RevNum,
    /// Symbolic names (tags and branch names) → the revision they name.
    pub symbols: Vec<(String, RevNum)>,
    /// The `expand` mode, if any (`b`/`o` mark binary; keyword expansion otherwise).
    pub expand: Option<String>,
    /// All revisions by number.
    pub revisions: BTreeMap<RevNum, Revision>,
}

impl RcsFile {
    /// Reconstruct a revision's full content by walking the delta path from `head`.
    ///
    /// # Errors
    /// [`Error::Read`] if the revision or its delta path is missing or malformed.
    pub fn content_of(&self, target: &RevNum) -> Result<Vec<u8>, Error> {
        let path = self.delta_path(target)?;
        let first = path
            .first()
            .ok_or_else(|| Error::Read("empty delta path".to_string()))?;
        let head_rev = self.rev(first)?;
        let mut lines = split_lines(&head_rev.text);
        for r in path.iter().skip(1) {
            let diff = &self.rev(r)?.text;
            lines = apply_diff(&lines, diff)?;
        }
        Ok(join_lines(&lines))
    }

    fn rev(&self, num: &RevNum) -> Result<&Revision, Error> {
        self.revisions
            .get(num)
            .ok_or_else(|| Error::Read(format!("revision {} not found", num.to_dotted())))
    }

    /// The chain `[head, …, target]` such that applying each successor's stored diff transforms the running
    /// content from `head` to `target`.
    fn delta_path(&self, target: &RevNum) -> Result<Vec<RevNum>, Error> {
        if target.is_trunk() {
            // Walk from head down the `next` chain until target.
            let mut chain = Vec::new();
            let mut cur = self.head.clone();
            for _ in 0..MAX_CHAIN {
                chain.push(cur.clone());
                if cur == *target {
                    return Ok(chain);
                }
                match &self.rev(&cur)?.next {
                    Some(n) => cur = n.clone(),
                    None => {
                        return Err(Error::Read(format!(
                            "trunk revision {} not reachable from head {}",
                            target.to_dotted(),
                            self.head.to_dotted()
                        )));
                    }
                }
            }
            Err(Error::Read(
                "delta chain too long (malformed ,v)".to_string(),
            ))
        } else {
            // A branch revision: reconstruct the branch point, then walk forward along the branch.
            let n = target.0.len();
            let bp = RevNum(target.0.get(..n - 2).unwrap_or(&[]).to_vec());
            let mut path = self.delta_path(&bp)?;
            let branch_prefix = target.0.get(..n - 1).unwrap_or(&[]);
            let bhead = self
                .rev(&bp)?
                .branches
                .iter()
                .find(|b| b.0.len() >= n - 1 && b.0.get(..n - 1) == Some(branch_prefix))
                .cloned()
                .ok_or_else(|| {
                    Error::Read(format!(
                        "branch for revision {} not found at branch point {}",
                        target.to_dotted(),
                        bp.to_dotted()
                    ))
                })?;
            let mut cur = bhead;
            for _ in 0..MAX_CHAIN {
                path.push(cur.clone());
                if cur == *target {
                    return Ok(path);
                }
                match &self.rev(&cur)?.next {
                    Some(x) => cur = x.clone(),
                    None => {
                        return Err(Error::Read(format!(
                            "branch revision {} not reachable",
                            target.to_dotted()
                        )));
                    }
                }
            }
            Err(Error::Read(
                "branch chain too long (malformed ,v)".to_string(),
            ))
        }
    }
}

/// A revision's delta-section metadata (parsed before its `log`/`text` in the deltatext section).
struct DeltaMeta {
    date: i64,
    author: String,
    state: String,
    branches: Vec<RevNum>,
    next: Option<RevNum>,
}

// ---- line utilities ------------------------------------------------------------------------------

/// Split into lines, each keeping its trailing `\n` (the last line may have none).
fn split_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            if let Some(line) = bytes.get(start..=i) {
                out.push(line.to_vec());
            }
            start = i + 1;
        }
    }
    if start < bytes.len() {
        if let Some(line) = bytes.get(start..) {
            out.push(line.to_vec());
        }
    }
    out
}

fn join_lines(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        out.extend_from_slice(l);
    }
    out
}

enum DiffCmd {
    /// Delete `count` source lines starting at 1-based `start`.
    Delete { start: usize, count: usize },
    /// After 1-based source line `after`, insert `lines`.
    Add { after: usize, lines: Vec<Vec<u8>> },
}

/// Parse and apply an ed-style RCS diff (a sequence of `a<n> <count>` / `d<n> <count>` commands, the added
/// lines following an `a`) to `source` lines.
fn apply_diff(source: &[Vec<u8>], diff: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    let script = split_lines(diff);
    let mut cmds = Vec::new();
    let mut i = 0usize;
    while i < script.len() {
        let line = script
            .get(i)
            .ok_or_else(|| Error::Read("diff underrun".to_string()))?;
        let s = std::str::from_utf8(line)
            .map_err(|_| Error::Read("RCS diff command is not UTF-8".to_string()))?
            .trim_end_matches('\n');
        if s.is_empty() {
            i += 1;
            continue;
        }
        let op = s.as_bytes().first().copied();
        let rest = s.get(1..).unwrap_or("");
        let mut it = rest.split_whitespace();
        let n = it
            .next()
            .and_then(|x| x.parse::<usize>().ok())
            .ok_or_else(|| Error::Read(format!("bad RCS diff command: {s}")))?;
        let count = it
            .next()
            .and_then(|x| x.parse::<usize>().ok())
            .ok_or_else(|| Error::Read(format!("bad RCS diff count: {s}")))?;
        match op {
            Some(b'd') => {
                cmds.push(DiffCmd::Delete { start: n, count });
                i += 1;
            }
            Some(b'a') => {
                let mut lines = Vec::with_capacity(count);
                for k in 0..count {
                    let l = script.get(i + 1 + k).ok_or_else(|| {
                        Error::Read("RCS diff 'a' ran past its lines".to_string())
                    })?;
                    lines.push(l.clone());
                }
                cmds.push(DiffCmd::Add { after: n, lines });
                i += 1 + count;
            }
            _ => return Err(Error::Read(format!("unknown RCS diff command: {s}"))),
        }
    }

    // Apply commands (which reference original-source line numbers, in increasing order).
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut src = 0usize; // count of source lines already consumed (0-based)
    for cmd in cmds {
        match cmd {
            DiffCmd::Delete { start, count } => {
                let stop = start.saturating_sub(1);
                while src < stop {
                    if let Some(l) = source.get(src) {
                        out.push(l.clone());
                    }
                    src += 1;
                }
                src = src.saturating_add(count);
            }
            DiffCmd::Add { after, lines } => {
                while src < after {
                    if let Some(l) = source.get(src) {
                        out.push(l.clone());
                    }
                    src += 1;
                }
                out.extend(lines);
            }
        }
    }
    while src < source.len() {
        if let Some(l) = source.get(src) {
            out.push(l.clone());
        }
        src += 1;
    }
    Ok(out)
}

// ---- the parser ----------------------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    /// Read a token: a run up to whitespace or one of `;:@`. May be empty.
    fn token(&mut self) -> String {
        self.skip_ws();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b';' | b':' | b'@') {
                break;
            }
            self.pos += 1;
        }
        String::from_utf8_lossy(self.data.get(start..self.pos).unwrap_or(&[])).into_owned()
    }

    fn expect(&mut self, ch: u8) -> Result<(), Error> {
        self.skip_ws();
        if self.peek() == Some(ch) {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::Read(format!("expected '{}' in ,v file", ch as char)))
        }
    }

    /// Read an `@`-delimited string, unescaping `@@` → `@`.
    fn string(&mut self) -> Result<Vec<u8>, Error> {
        self.skip_ws();
        if self.peek() != Some(b'@') {
            return Err(Error::Read("expected an @-delimited string".to_string()));
        }
        self.pos += 1;
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if b == b'@' {
                // Look ahead: `@@` is an escaped `@`; a lone `@` closes the string.
                if self.data.get(self.pos + 1).copied() == Some(b'@') {
                    out.push(b'@');
                    self.pos += 2;
                } else {
                    self.pos += 1;
                    return Ok(out);
                }
            } else {
                out.push(b);
                self.pos += 1;
            }
        }
        Err(Error::Read("unterminated @-string in ,v file".to_string()))
    }

    /// Consume tokens/strings until the next `;` (for skipping a field or a newphrase).
    fn skip_to_semi(&mut self) -> Result<(), Error> {
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b';') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some(b'@') => {
                    self.string()?;
                }
                Some(b':') => {
                    self.pos += 1;
                }
                None => {
                    return Err(Error::Read(
                        "unexpected end of ,v (looking for ';')".to_string(),
                    ));
                }
                _ => {
                    let t = self.token();
                    if t.is_empty() {
                        // A stray delimiter we don't handle; advance to avoid a loop.
                        self.pos += 1;
                    }
                }
            }
        }
    }
}

/// Parse an RCS `,v` file.
///
/// # Errors
/// [`Error::Read`] on malformed structure; [`Error::ResourceLimit`] if the file exceeds [`MAX_RCS_BYTES`].
pub fn parse_rcs(data: &[u8]) -> Result<RcsFile, Error> {
    if data.len() > MAX_RCS_BYTES {
        return Err(Error::ResourceLimit {
            limit: format!("RCS file over {MAX_RCS_BYTES} bytes"),
        });
    }
    let mut cur = Cursor::new(data);
    let mut head: Option<RevNum> = None;
    let mut symbols: Vec<(String, RevNum)> = Vec::new();
    let mut expand: Option<String> = None;

    // ---- admin section: keyword-led fields until the first delta (a numeric token). ----
    loop {
        cur.skip_ws();
        let save = cur.pos;
        let kw = cur.token();
        if kw.is_empty() {
            return Err(Error::Read("empty token in ,v admin section".to_string()));
        }
        if kw.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            // Not a keyword — the delta section begins here.
            cur.pos = save;
            break;
        }
        match kw.as_str() {
            "head" => {
                let n = cur.token();
                head = RevNum::parse(&n);
                cur.expect(b';')?;
            }
            "symbols" => {
                // pairs `name:rev` until `;`
                loop {
                    cur.skip_ws();
                    if cur.peek() == Some(b';') {
                        cur.pos += 1;
                        break;
                    }
                    let name = cur.token();
                    if name.is_empty() {
                        cur.pos += 1;
                        continue;
                    }
                    cur.expect(b':')?;
                    let rev = cur.token();
                    if let Some(r) = RevNum::parse(&rev) {
                        symbols.push((name, r));
                    }
                }
            }
            "expand" => {
                let s = cur.string()?;
                expand = Some(String::from_utf8_lossy(&s).into_owned());
                cur.expect(b';')?;
            }
            // `branch`, `access`, `locks` (+ optional `strict`), `comment`, and any newphrase: skip to `;`.
            "strict" => {
                cur.expect(b';')?;
            }
            _ => {
                cur.skip_to_semi()?;
            }
        }
    }

    let head = head.ok_or_else(|| Error::Read("no head revision in ,v file".to_string()))?;

    // ---- delta section: metadata per revision, until `desc`. ----
    let mut deltas: BTreeMap<RevNum, DeltaMeta> = BTreeMap::new();
    loop {
        cur.skip_ws();
        let save = cur.pos;
        let tok = cur.token();
        if tok == "desc" {
            cur.pos = save;
            break;
        }
        let num = RevNum::parse(&tok)
            .ok_or_else(|| Error::Read(format!("expected a revision number, got '{tok}'")))?;
        let mut date = 0i64;
        let mut author = String::new();
        let mut state = String::new();
        let mut branches: Vec<RevNum> = Vec::new();
        let mut next: Option<RevNum> = None;
        // fields until we reach the next revision number or `desc`
        loop {
            cur.skip_ws();
            let fsave = cur.pos;
            let field = cur.token();
            match field.as_str() {
                "date" => {
                    let v = cur.token();
                    date = parse_rcs_date(&v).unwrap_or(0);
                    cur.expect(b';')?;
                }
                "author" => {
                    author = cur.token();
                    cur.expect(b';')?;
                }
                "state" => {
                    state = cur.token();
                    cur.expect(b';')?;
                }
                "branches" => loop {
                    cur.skip_ws();
                    if cur.peek() == Some(b';') {
                        cur.pos += 1;
                        break;
                    }
                    let b = cur.token();
                    if b.is_empty() {
                        cur.pos += 1;
                        continue;
                    }
                    if let Some(r) = RevNum::parse(&b) {
                        branches.push(r);
                    }
                },
                "next" => {
                    let n = cur.token();
                    next = RevNum::parse(&n);
                    cur.expect(b';')?;
                }
                _ => {
                    // Either a newphrase (skip to ;) or the start of the next delta / desc.
                    if field == "desc"
                        || field.as_bytes().first().is_some_and(u8::is_ascii_digit)
                        || field.is_empty()
                    {
                        cur.pos = fsave;
                        break;
                    }
                    cur.skip_to_semi()?;
                }
            }
        }
        deltas.insert(
            num,
            DeltaMeta {
                date,
                author,
                state,
                branches,
                next,
            },
        );
    }

    // ---- desc ----
    let _desc = {
        let kw = cur.token(); // "desc"
        if kw != "desc" {
            return Err(Error::Read(format!("expected 'desc', got '{kw}'")));
        }
        cur.string()?
    };

    // ---- deltatext section: log + text per revision. ----
    let mut texts: BTreeMap<RevNum, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
    loop {
        cur.skip_ws();
        if cur.peek().is_none() {
            break;
        }
        let tok = cur.token();
        if tok.is_empty() {
            break;
        }
        let num = RevNum::parse(&tok).ok_or_else(|| {
            Error::Read(format!(
                "expected a revision number in deltatext, got '{tok}'"
            ))
        })?;
        let mut log = Vec::new();
        let mut text = Vec::new();
        // fields `log <string>` and `text <string>` in some order
        for _ in 0..2 {
            cur.skip_ws();
            let field = cur.token();
            match field.as_str() {
                "log" => log = cur.string()?,
                "text" => text = cur.string()?,
                other => return Err(Error::Read(format!("expected log/text, got '{other}'"))),
            }
        }
        texts.insert(num, (log, text));
    }

    // ---- assemble ----
    let mut revisions = BTreeMap::new();
    for (num, meta) in deltas {
        let (log, text) = texts.get(&num).cloned().unwrap_or_default();
        revisions.insert(
            num.clone(),
            Revision {
                num,
                date: meta.date,
                author: meta.author,
                state: meta.state,
                branches: meta.branches,
                next: meta.next,
                log,
                text,
            },
        );
    }

    Ok(RcsFile {
        head,
        symbols,
        expand,
        revisions,
    })
}

/// Parse an RCS date (`YYYY.MM.DD.hh.mm.ss`, 2-digit years pre-2000) to epoch seconds.
fn parse_rcs_date(s: &str) -> Option<i64> {
    let mut it = s.split('.');
    let mut year = it.next()?.parse::<i64>().ok()?;
    if year < 100 {
        year += 1900;
    }
    let month = it.next()?.parse::<i64>().ok()?;
    let day = it.next()?.parse::<i64>().ok()?;
    let hour = it.next()?.parse::<i64>().ok()?;
    let min = it.next()?.parse::<i64>().ok()?;
    let sec = it.next()?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day).checked_mul(86_400)? + hour * 3_600 + min * 60 + sec)
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + if m > 2 { -3 } else { 9 }) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests;
