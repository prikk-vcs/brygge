//! Corpus generators for the 0.2.0 measurement scenarios (`git-commits`, `git-content`, `cvs-revs`,
//! `svn-dump`). Each is synthetic and deterministic: the same `n` always produces byte-identical corpora, and
//! each returns the exact IR statistics the decode must reproduce (the scenario's self-check).

use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// The IR statistics a decode must reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub atoms: usize,
    pub blobs: usize,
    pub bytes: u64,
}

/// `len` bytes of deterministic pseudo-text (lowercase letters, a newline every 64 bytes), from `seed`. Different
/// seeds give different content, so blobs are distinct; the same seed always gives the same bytes.
pub fn fill(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.push(if i % 64 == 63 {
            b'\n'
        } else {
            b'a' + ((r >> 33) % 26) as u8
        });
    }
    out
}

// ---- Git (via `git fast-import`) -----------------------------------------------------------------

const GIT_TREE_FILES: u64 = 500;
const GIT_CONTENT_SIZE: usize = 8192;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Initialise a repository in `dir` and feed it a `git fast-import` stream written by `stream`. The stream is
/// written incrementally, so the generator's own memory stays small (the measurement is the decoder's).
fn fast_import(dir: &Path, stream: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) {
    git(dir, &["init", "-q", "--initial-branch=main"]);
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(["fast-import", "--quiet"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn git fast-import");
    {
        let stdin = child.stdin.take().expect("fast-import stdin");
        let mut w = BufWriter::with_capacity(1 << 20, stdin);
        stream(&mut w).expect("write the fast-import stream");
        w.flush().expect("flush the fast-import stream");
    }
    assert!(
        child.wait().expect("wait").success(),
        "git fast-import failed"
    );
}

fn commit_header(w: &mut dyn Write, ts: u64, msg: &str) -> std::io::Result<()> {
    writeln!(w, "commit refs/heads/main")?;
    writeln!(w, "author Bench <bench@example.com> {ts} +0000")?;
    writeln!(w, "committer Bench <bench@example.com> {ts} +0000")?;
    write!(w, "data {}\n{msg}\n", msg.len())
}

fn modify(w: &mut dyn Write, path: &str, content: &[u8]) -> std::io::Result<()> {
    write!(w, "M 100644 inline {path}\ndata {}\n", content.len())?;
    w.write_all(content)?;
    w.write_all(b"\n")
}

fn tree_path(i: u64) -> String {
    format!("d{}/f{i}.txt", i % 20)
}

/// `git-commits <n>`: a fixed tree of 500 files at the first commit, then `n` commits each editing one file.
/// `n + 1` commits and `500 + n` distinct blobs. Every commit makes a new root tree (the snapshot cache grows).
pub fn git_commits(dir: &Path, n: u64) -> Stats {
    let mut bytes = 0u64;
    fast_import(dir, |w| {
        commit_header(w, 1_700_000_000, "tree")?;
        for i in 0..GIT_TREE_FILES {
            let content = format!("v0 of {i}\n");
            bytes += content.len() as u64;
            modify(w, &tree_path(i), content.as_bytes())?;
        }
        w.write_all(b"\n")?;
        for r in 0..n {
            commit_header(w, 1_700_000_060 + r * 60, "edit")?;
            let i = r % GIT_TREE_FILES;
            let content = format!("edit {r} of {i}\n");
            bytes += content.len() as u64;
            modify(w, &tree_path(i), content.as_bytes())?;
            w.write_all(b"\n")?;
        }
        Ok(())
    });
    Stats {
        atoms: usize::try_from(n + 1).expect("n fits"),
        blobs: usize::try_from(GIT_TREE_FILES + n).expect("n fits"),
        bytes,
    }
}

/// `git-content <n>`: three commits and `n` sizeable (8 KiB) files: one commit adds them all, one rewrites every
/// tenth, one adds a README. Peak should track content, not history.
pub fn git_content(dir: &Path, n: u64) -> Stats {
    let readme = b"bench\n";
    fast_import(dir, |w| {
        commit_header(w, 1_700_000_000, "add")?;
        for i in 0..n {
            modify(w, &tree_path(i), &fill(i, GIT_CONTENT_SIZE))?;
        }
        w.write_all(b"\n")?;
        commit_header(w, 1_700_000_060, "rewrite every tenth")?;
        for i in (0..n).step_by(10) {
            modify(w, &tree_path(i), &fill(1_000_000_000 + i, GIT_CONTENT_SIZE))?;
        }
        w.write_all(b"\n")?;
        commit_header(w, 1_700_000_120, "readme")?;
        modify(w, "README", readme)?;
        w.write_all(b"\n")
    });
    let rewritten = n.div_ceil(10);
    Stats {
        atoms: 3,
        blobs: usize::try_from(n + rewritten + 1).expect("n fits"),
        bytes: (n + rewritten) * GIT_CONTENT_SIZE as u64 + readme.len() as u64,
    }
}

// ---- SVN dump, streamed to a file -----------------------------------------------------------------

const SVN_FILES: u64 = 300;
const SVN_CONTENT_SIZE: usize = 2048;

/// `svn-dump <n>`: an SVN dump over 300 files of 2 KiB: r0 (empty), r1 (the `trunk` directory), r2 (the files),
/// then `n` revisions each rewriting one file with new content. Every revision, r0 included, is an atom. The dump is written revision by revision. The dump is
/// content-heavy (unlike `svn-revs`), so the parsed `Dump` held beside the IR (increment 2) is measurable.
pub fn svn_dump(path: &Path, n: u64) -> Stats {
    use crate::{svn_add_dir, svn_add_file as add_file, svn_change_file, svn_header, svn_revision};
    let file = std::fs::File::create(path).expect("create the dump");
    let mut w = BufWriter::with_capacity(1 << 20, file);
    let mut b = Vec::new();
    svn_header(&mut b);
    svn_revision(&mut b, 0, "init");
    svn_revision(&mut b, 1, "trunk");
    svn_add_dir(&mut b, "trunk", None);
    w.write_all(&b).expect("write");
    b.clear();
    svn_revision(&mut b, 2, "add files");
    for i in 0..SVN_FILES {
        add_file(
            &mut b,
            &format!("trunk/f{i}.txt"),
            &fill(i, SVN_CONTENT_SIZE),
        );
    }
    w.write_all(&b).expect("write");
    for r in 0..n {
        b.clear();
        svn_revision(&mut b, r + 3, "edit");
        svn_change_file(
            &mut b,
            &format!("trunk/f{}.txt", r % SVN_FILES),
            &fill(1_000_000_000 + r, SVN_CONTENT_SIZE),
        );
        w.write_all(&b).expect("write");
    }
    w.flush().expect("flush");
    Stats {
        atoms: usize::try_from(n + 3).expect("n fits"),
        blobs: usize::try_from(SVN_FILES + n).expect("n fits"),
        bytes: (SVN_FILES + n) * SVN_CONTENT_SIZE as u64,
    }
}

// ---- CVS: long trunk delta chains ------------------------------------------------------------------

pub const CVS_FILES: u64 = 20;
const CVS_LINES: usize = 200;

/// One file's evolution. Revision 1 has every line at revision 1; revision `k >= 2` rewrites three lines
/// (same length class, so a delta is a replace of each), and a line's text names the last revision that
/// wrote it. So every revision's text is distinct, and each reverse delta is three replaces.
pub struct FileModel {
    file: u64,
    rev_of_line: Vec<u64>,
    /// The revision the model is at (1-based).
    pub rev: u64,
}

impl FileModel {
    pub fn new(file: u64) -> Self {
        Self {
            file,
            rev_of_line: vec![1; CVS_LINES],
            rev: 1,
        }
    }

    fn line(&self, l: usize, rev: u64) -> String {
        format!("line {l} of file {} rev {rev}\n", self.file)
    }

    /// The lines revision `k` rewrites, ascending and distinct.
    fn edited(k: u64) -> Vec<usize> {
        let l = CVS_LINES as u64;
        let mut v = vec![
            ((k * 7) % l) as usize,
            ((k * 13 + 1) % l) as usize,
            ((k * 29 + 2) % l) as usize,
        ];
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Advance to the next revision; returns the lines it rewrote with each one's previous revision.
    pub fn advance(&mut self) -> Vec<(usize, u64)> {
        self.rev += 1;
        let mut prev = Vec::new();
        for l in Self::edited(self.rev) {
            prev.push((l, self.rev_of_line[l]));
            self.rev_of_line[l] = self.rev;
        }
        prev
    }

    /// The current revision's full text.
    pub fn text(&self) -> Vec<u8> {
        let mut out = String::new();
        for l in 0..CVS_LINES {
            out.push_str(&self.line(l, self.rev_of_line[l]));
        }
        out.into_bytes()
    }
}

/// The date of revision `k` of file `i`: one hour apart across all files (so no two revisions cluster into one
/// changeset), as `YYYY.MM.DD.HH.MM.SS`, from 2000-01-01.
fn cvs_date(i: u64, k: u64) -> String {
    let secs = (k * CVS_FILES + i) * 3600;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // civil-from-days (Howard Hinnant), for days since 2000-01-01 (= 10957 since 1970-01-01).
    let z = i64::try_from(days).expect("days fit") + 10_957 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}.{m:02}.{d:02}.{:02}.{:02}.{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// The `,v` text of file `i` with `n` trunk revisions 1.1 … 1.n: the head in full and each older revision as a
/// reverse delta, exactly as RCS stores it. Returns the file and the total bytes of all `n` revision texts.
fn cvs_file(i: u64, n: u64) -> (Vec<u8>, u64) {
    let mut model = FileModel::new(i);
    let mut total = model.text().len() as u64;
    // edits[k] (k >= 2): what revision k rewrote, with each line's previous revision.
    let mut edits: Vec<Vec<(usize, u64)>> = vec![Vec::new(), Vec::new()];
    for _ in 2..=n {
        edits.push(model.advance());
        total += model.text().len() as u64;
    }
    let head = model.text();

    let mut s = String::new();
    s.push_str(&format!(
        "head\t1.{n};\naccess;\nsymbols;\nlocks; strict;\n\n\n"
    ));
    for k in (1..=n).rev() {
        let next = if k > 1 {
            format!("1.{}", k - 1)
        } else {
            String::new()
        };
        s.push_str(&format!(
            "1.{k}\ndate\t{};\tauthor a;\tstate Exp;\nbranches;\nnext\t{next};\n\n",
            cvs_date(i, k)
        ));
    }
    s.push_str("\ndesc\n@@\n\n\n");
    // The head's delta text is its full text.
    s.push_str(&format!(
        "1.{n}\nlog\n@file{i} rev {n}@\ntext\n@{}@\n",
        String::from_utf8(head).expect("ascii")
    ));
    // Revision k from revision k+1: put every line that k+1 rewrote back to its previous text.
    for k in (1..n).rev() {
        let mut delta = String::new();
        for &(l, prev_rev) in &edits[usize::try_from(k + 1).expect("k fits")] {
            let line1 = l + 1;
            delta.push_str(&format!("d{line1} 1\na{line1} 1\n"));
            delta.push_str(&format!("line {l} of file {i} rev {prev_rev}\n"));
        }
        s.push_str(&format!(
            "\n1.{k}\nlog\n@file{i} rev {k}@\ntext\n@{delta}@\n"
        ));
    }
    (s.into_bytes(), total)
}

/// `cvs-revs <n>`: 20 `,v` files, each with `n` trunk revisions, each revision rewriting three of 200 lines.
/// One atom per revision (`20 n`), every content distinct. Today's reconstruction re-walks the chain from the
/// head for each revision, O(revisions²) per file.
pub fn cvs_revs(dir: &Path, n: u64) -> Stats {
    let mut bytes = 0u64;
    for i in 0..CVS_FILES {
        let (v, total) = cvs_file(i, n);
        bytes += total;
        std::fs::write(dir.join(format!("f{i}.c,v")), v).expect("write ,v");
    }
    Stats {
        atoms: usize::try_from(CVS_FILES * n).expect("n fits"),
        blobs: usize::try_from(CVS_FILES * n).expect("n fits"),
        bytes,
    }
}
