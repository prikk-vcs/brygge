//! Dev-only measurement harness for brygge's decoders (RFC 010 OQ-F).
//!
//! Measures **peak resident memory** and **wall time** decoding synthetic corpora at increasing scale, so
//! the streaming increments (RFC 010) are gated on evidence rather than intuition. Zero new dependencies
//! and zero `unsafe`: peak memory is read from `/proc/self/status` (`VmHWM`, the process high-water mark)
//! on Linux, and each scenario runs in its **own subprocess** so the high-water mark reflects that one
//! decode alone. On non-Linux the memory column reads `n/a` (time and IR stats still report).
//!
//! Usage:
//!   brygge-bench                      # run the full matrix and print a table
//!   brygge-bench run <scenario> <n>   # run one scenario at scale n; print a machine line (used internally)
//!   brygge-bench corpus <scenario> <n> <dir>  # write a scenario's corpus into <dir> and stop (for A/B with the CLI)
//!
//! Scenarios:
//!   svn-revs <n>     — an SVN dump of ~n revisions with tiny per-revision content and one branch copy at
//!                      r1. Content is ~constant, so peak should track *scratch*, not revision count —
//!                      the RFC 010 increment-1 property (snapshot retention no longer grows with revisions).
//!   svn-content <n>  — an SVN dump of a few revisions but n sizeable files. Peak should grow with content
//!                      (the IR floor: the IR *is* the content).
//!   cvs <n>          — a CVS repository of n single-revision files. Peak/time for the CVS decode path.
//!
//! The 0.2.0 scenarios (RFC 010 OQ-A: measure before building), each with a self-check that the decode
//! reproduces the generator's IR statistics exactly:
//!   git-commits <n>  — a 500-file tree, then n commits each editing one file (increment 5: the Git snapshot
//!                      cache grows with distinct root trees).
//!   git-content <n>  — three commits and n sizeable files (the IR floor for Git: peak should track content).
//!   cvs-revs <n>     — 20 `,v` files each with n trunk revisions (increment 3: the O(revisions²) per-file
//!                      reconstruction); also checks every revision's content against the generator's text.
//!   cvs-branches <n> — the `cvs-revs` files plus a branch of n/4 revisions and a nested branch of n/8 (RFC 013
//!                      C-2), decoded with `--reconstruct-refs`; `cvs-branches-plain` decodes the same files
//!                      without it (the trunk alone). Both check the IR statistics against the generator's.
//!   svn-dump <n>     — a content-heavy SVN dump of n revisions over 300 files (increment 2: the parsed dump held
//!                      beside the IR).

// A dev-only tool: unwrap/expect/indexing are acceptable here and keep the generators readable.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod corpus;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use brygge_decode_cvs::{Options as CvsOpts, Source as CvsSource};
use brygge_decode_git::Options as GitOpts;
use brygge_decode_svn::{Options as SvnOpts, Source as SvnSource};
use brygge_ir::Ir;
use brygge_ir::model::PathOp;

use corpus::Stats;

const SCENARIOS: &[(&str, &[u64])] = &[
    ("svn-revs", &[1_000, 5_000, 20_000]),
    ("svn-content", &[200, 1_000, 4_000]),
    ("cvs", &[200, 1_000, 4_000]),
    ("git-commits", &[1_000, 5_000, 20_000]),
    ("git-content", &[1_000, 5_000]),
    ("cvs-revs", &[200, 1_000, 5_000]),
    ("svn-dump", &[1_000, 5_000, 20_000]),
    ("cvs-branches", &[200, 1_000, 5_000]),
    ("cvs-branches-plain", &[200, 1_000, 5_000]),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let scenario = args.get(2).map(String::as_str).unwrap_or("");
            let n: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            run_one(scenario, n);
        }
        Some("corpus") => {
            // Write a scenario's corpus and stop (no decode): so the same input can be decoded by two builds
            // of the `brygge` CLI and the artifacts compared byte for byte (the A/B of a byte-identical change).
            let scenario = args.get(2).map(String::as_str).unwrap_or("");
            let n: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            let Some(dir) = args.get(4).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench corpus <scenario> <n> <dir>");
                std::process::exit(2);
            };
            std::fs::create_dir_all(&dir).expect("create the corpus directory");
            let Some(corpus) = prepare(scenario, n, &dir) else {
                eprintln!("unknown scenario: {scenario}");
                std::process::exit(2);
            };
            let (kind, path) = match &corpus.source {
                Source::Svn(p) => ("svn", p),
                Source::Cvs(p) | Source::CvsRefs(p) => ("cvs", p),
                Source::Git(p) => ("git", p),
            };
            println!("{kind} {}", path.display());
        }
        _ => run_matrix(),
    }
}

/// Run every (scenario, scale) as a subprocess and print a table. A scenario whose self-check fails (or that
/// crashes) prints `FAILED`, and the run exits non-zero.
fn run_matrix() {
    println!("brygge decoder memory/time harness (RFC 010 OQ-F)");
    println!(
        "{:<14} {:>8} {:>10} {:>9} {:>8} {:>8} {:>12}  check",
        "scenario", "scale", "peak_KiB", "time_ms", "atoms", "blobs", "content_B"
    );
    let exe = std::env::current_exe().expect("current exe");
    let mut failed = false;
    for (scenario, scales) in SCENARIOS {
        for &n in *scales {
            let out = Command::new(&exe)
                .args(["run", scenario, &n.to_string()])
                .output()
                .expect("spawn self");
            let line = String::from_utf8_lossy(&out.stdout);
            let f = parse(&line);
            let ok = out.status.success() && f.get("check").is_none_or(|c| c == "ok");
            failed |= !ok;
            println!(
                "{:<14} {:>8} {:>10} {:>9} {:>8} {:>8} {:>12}  {}",
                scenario,
                n,
                f.get("peak_kb").cloned().unwrap_or_else(|| "n/a".into()),
                f.get("time_ms").cloned().unwrap_or_default(),
                f.get("atoms").cloned().unwrap_or_default(),
                f.get("blobs").cloned().unwrap_or_default(),
                f.get("bytes").cloned().unwrap_or_default(),
                if ok {
                    f.get("check").cloned().unwrap_or_default()
                } else {
                    "FAILED".into()
                },
            );
            if !ok {
                eprintln!("{}", String::from_utf8_lossy(&out.stderr));
            }
        }
    }
    println!(
        "\nreading: svn-revs peak should stay ~flat as scale grows (scratch bounded, RFC 010 inc.1);\n\
         svn-content peak should grow with scale (the IR floor — the IR is the content).\n\
         The 0.2.0 scenarios: see README, \"0.2.0 baseline\"."
    );
    if failed {
        std::process::exit(1);
    }
}

/// What a scenario decodes, and what the decode must reproduce.
struct Corpus {
    source: Source,
    /// The IR statistics the generator computed; `None` for the older scenarios, which only report them.
    expected: Option<Stats>,
    /// `cvs-revs`: also check every revision's content against the generator's text.
    cvs_content_check: bool,
}

enum Source {
    Svn(PathBuf),
    Cvs(PathBuf),
    /// CVS, decoded with `--reconstruct-refs` (the branches).
    CvsRefs(PathBuf),
    Git(PathBuf),
}

/// Decode one scenario at scale n, then print a machine line with its peak and stats. Only the decode is timed
/// and its peak measured: the corpus is written first (streamed, where large), and the high-water mark is reset
/// before the decode starts.
fn run_one(scenario: &str, n: u64) {
    let dir = std::env::temp_dir().join(format!(
        "brygge-bench-{}-{scenario}-{n}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);

    let Some(corpus) = prepare(scenario, n, &dir) else {
        eprintln!("unknown scenario: {scenario}");
        std::process::exit(2);
    };

    reset_peak_rss();
    let start = Instant::now();
    let ir = decode(&corpus.source);
    let time_ms = start.elapsed().as_millis();
    let peak = peak_rss_kib().map_or_else(|| "n/a".to_string(), |k| k.to_string());

    let got = Stats {
        atoms: ir.atoms.len(),
        blobs: ir.content.len(),
        bytes: ir.content.total_bytes(),
    };
    let check = self_check(&corpus, &ir, got);
    let _ = std::fs::remove_dir_all(&dir);
    println!(
        "peak_kb={peak} time_ms={time_ms} atoms={} blobs={} bytes={} check={}",
        got.atoms,
        got.blobs,
        got.bytes,
        if check.is_ok() { "ok" } else { "FAILED" }
    );
    if let Err(why) = check {
        eprintln!("self-check failed for {scenario} {n}: {why}");
        std::process::exit(1);
    }
}

/// Write the corpus for `scenario` at scale `n` into `dir`.
fn prepare(scenario: &str, n: u64, dir: &Path) -> Option<Corpus> {
    Some(match scenario {
        "svn-revs" => Corpus {
            source: write_svn(dir, svn_revs_dump(n)),
            expected: None,
            cvs_content_check: false,
        },
        "svn-content" => Corpus {
            source: write_svn(dir, svn_content_dump(n)),
            expected: None,
            cvs_content_check: false,
        },
        "cvs" => {
            for i in 0..n {
                let vfile = cvs_single_rev(&format!("commit {i}"), &format!("body of file {i}\n"));
                std::fs::write(dir.join(format!("f{i}.c,v")), vfile).unwrap();
            }
            Corpus {
                source: Source::Cvs(dir.to_path_buf()),
                expected: None,
                cvs_content_check: false,
            }
        }
        "git-commits" => Corpus {
            expected: Some(corpus::git_commits(dir, n)),
            source: Source::Git(dir.to_path_buf()),
            cvs_content_check: false,
        },
        "git-content" => Corpus {
            expected: Some(corpus::git_content(dir, n)),
            source: Source::Git(dir.to_path_buf()),
            cvs_content_check: false,
        },
        "cvs-revs" => Corpus {
            expected: Some(corpus::cvs_revs(dir, n)),
            source: Source::Cvs(dir.to_path_buf()),
            cvs_content_check: true,
        },
        "cvs-branches" => Corpus {
            expected: Some(corpus::cvs_branches(dir, n).0),
            source: Source::CvsRefs(dir.to_path_buf()),
            cvs_content_check: false,
        },
        "cvs-branches-plain" => Corpus {
            expected: Some(corpus::cvs_branches(dir, n).1),
            source: Source::Cvs(dir.to_path_buf()),
            cvs_content_check: false,
        },
        "svn-dump" => {
            let path = dir.join("in.dump");
            let expected = corpus::svn_dump(&path, n);
            Corpus {
                source: Source::Svn(path),
                expected: Some(expected),
                cvs_content_check: false,
            }
        }
        _ => return None,
    })
}

fn write_svn(dir: &Path, dump: Vec<u8>) -> Source {
    let path = dir.join("in.dump");
    std::fs::write(&path, &dump).unwrap();
    drop(dump); // do not hold the generated bytes across the decode
    Source::Svn(path)
}

fn decode(source: &Source) -> Ir {
    match source {
        Source::Svn(path) => {
            brygge_decode_svn::decode(&SvnSource::DumpFile(path.clone()), &SvnOpts::default())
                .expect("svn decode")
        }
        Source::Cvs(dir) => {
            brygge_decode_cvs::decode(&CvsSource::LocalRepo(dir.clone()), &CvsOpts::default())
                .expect("cvs decode")
        }
        Source::CvsRefs(dir) => {
            let opts = CvsOpts {
                reconstruct_refs: true,
                ..CvsOpts::default()
            };
            brygge_decode_cvs::decode(&CvsSource::LocalRepo(dir.clone()), &opts)
                .expect("cvs decode")
        }
        Source::Git(dir) => {
            brygge_decode_git::decode(dir, &GitOpts::default()).expect("git decode")
        }
    }
}

/// The scenario's self-check: the IR statistics equal the generator's, and (`cvs-revs`) every revision of every
/// file has exactly the content the generator intended.
fn self_check(corpus: &Corpus, ir: &Ir, got: Stats) -> Result<(), String> {
    if let Some(expected) = corpus.expected {
        if expected != got {
            return Err(format!(
                "the IR has {got:?}, the generator intended {expected:?}"
            ));
        }
    }
    if corpus.cvs_content_check {
        check_cvs_revs_content(ir)?;
    }
    Ok(())
}

/// `cvs-revs`: atoms are in time order and each holds one revision of one file, so the k-th atom touching
/// `f<i>.c` must carry revision k of that file's intended text.
fn check_cvs_revs_content(ir: &Ir) -> Result<(), String> {
    let mut models: std::collections::BTreeMap<u64, corpus::FileModel> =
        std::collections::BTreeMap::new();
    for (idx, atom) in ir.atoms.iter().enumerate() {
        if atom.ops.len() != 1 {
            return Err(format!("atom {idx} has {} ops, expected 1", atom.ops.len()));
        }
        let (path, blob) = match &atom.ops[0] {
            PathOp::Add { path, blob, .. } | PathOp::Modify { path, blob, .. } => (path, blob),
            other => return Err(format!("atom {idx}: unexpected op {other:?}")),
        };
        let file: u64 = path
            .strip_prefix('f')
            .and_then(|r| r.strip_suffix(".c"))
            .and_then(|r| r.parse().ok())
            .ok_or_else(|| format!("atom {idx}: unexpected path {path}"))?;
        let model = match models.get_mut(&file) {
            Some(m) => {
                m.advance();
                m
            }
            None => models
                .entry(file)
                .or_insert_with(|| corpus::FileModel::new(file)),
        };
        let want = model.text();
        let got = ir
            .content
            .get(blob)
            .ok_or_else(|| format!("atom {idx}: blob missing"))?;
        if got != want.as_slice() {
            return Err(format!(
                "atom {idx}: {path} revision 1.{} differs from the intended text",
                model.rev
            ));
        }
    }
    if models.len() as u64 != corpus::CVS_FILES {
        return Err(format!(
            "{} files seen, expected {}",
            models.len(),
            corpus::CVS_FILES
        ));
    }
    Ok(())
}

// ---- peak RSS (Linux /proc, no unsafe) -----------------------------------------------------------

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Reset the kernel's peak-RSS mark (`echo 5 > /proc/self/clear_refs`), so `VmHWM` afterwards reflects the
/// decode alone and not the corpus generation before it. Best effort; where it fails the peak is higher, never
/// lower, than the decode's own.
fn reset_peak_rss() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

// ---- SVN dumpstream generators -------------------------------------------------------------------

fn svn_header(buf: &mut Vec<u8>) {
    buf.extend(b"SVN-fs-dump-format-version: 2\n\n");
    buf.extend(b"UUID: 00000000-0000-0000-0000-000000000000\n\n");
}

fn props_block(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in pairs {
        b.extend(format!("K {}\n{k}\nV {}\n{v}\n", k.len(), v.len()).into_bytes());
    }
    b.extend(b"PROPS-END\n");
    b
}

fn svn_revision(buf: &mut Vec<u8>, num: u64, log: &str) {
    let p = props_block(&[
        ("svn:date", "2024-01-01T00:00:00.000000Z"),
        ("svn:log", log),
    ]);
    buf.extend(format!("Revision-number: {num}\n").into_bytes());
    buf.extend(
        format!(
            "Prop-content-length: {}\nContent-length: {}\n\n",
            p.len(),
            p.len()
        )
        .into_bytes(),
    );
    buf.extend(p);
    buf.extend(b"\n");
}

fn svn_add_file(buf: &mut Vec<u8>, path: &str, content: &[u8]) {
    let p = props_block(&[]);
    buf.extend(format!("Node-path: {path}\nNode-kind: file\nNode-action: add\n").into_bytes());
    push_file_body(buf, &p, content);
}

fn svn_change_file(buf: &mut Vec<u8>, path: &str, content: &[u8]) {
    let p = props_block(&[]);
    buf.extend(format!("Node-path: {path}\nNode-kind: file\nNode-action: change\n").into_bytes());
    push_file_body(buf, &p, content);
}

fn push_file_body(buf: &mut Vec<u8>, p: &[u8], content: &[u8]) {
    buf.extend(format!("Prop-content-length: {}\n", p.len()).into_bytes());
    buf.extend(format!("Text-content-length: {}\n", content.len()).into_bytes());
    buf.extend(format!("Content-length: {}\n\n", p.len() + content.len()).into_bytes());
    buf.extend(p);
    buf.extend(content);
    buf.extend(b"\n\n");
}

fn svn_add_dir(buf: &mut Vec<u8>, path: &str, copyfrom: Option<(u64, &str)>) {
    buf.extend(format!("Node-path: {path}\nNode-kind: dir\nNode-action: add\n").into_bytes());
    if let Some((r, p)) = copyfrom {
        buf.extend(format!("Node-copyfrom-rev: {r}\nNode-copyfrom-path: {p}\n").into_bytes());
    }
    buf.extend(b"\n\n");
}

/// A large fixed tree (`TREE_FILES` files) established at r1, one branch copy at r2 (pinning exactly one
/// snapshot), then ~n revisions each editing a single file. The tree is large so that the pre-increment-1
/// decoder would have retained O(revisions × TREE_FILES) snapshot entries; with the bound, only r1's
/// snapshot is kept, so peak should track the IR (initial tree + n edits), *not* revisions × tree.
fn svn_revs_dump(n: u64) -> Vec<u8> {
    const TREE_FILES: u64 = 500;
    let mut b = Vec::new();
    svn_header(&mut b);
    svn_revision(&mut b, 0, "init");
    svn_revision(&mut b, 1, "trunk");
    svn_add_dir(&mut b, "trunk", None);
    for i in 0..TREE_FILES {
        svn_add_file(
            &mut b,
            &format!("trunk/f{i}.txt"),
            format!("v0 of {i}\n").as_bytes(),
        );
    }
    svn_revision(&mut b, 2, "branch");
    svn_add_dir(&mut b, "branches", None);
    svn_add_dir(&mut b, "branches/x", Some((1, "trunk"))); // the only retained snapshot
    for r in 0..n {
        svn_revision(&mut b, r + 3, "edit");
        svn_change_file(
            &mut b,
            &format!("trunk/f{}.txt", r % TREE_FILES),
            format!("edit {r}\n").as_bytes(),
        );
    }
    b
}

/// A few revisions but n sizeable files — content grows with n (the IR floor).
fn svn_content_dump(n: u64) -> Vec<u8> {
    let mut b = Vec::new();
    svn_header(&mut b);
    svn_revision(&mut b, 0, "init");
    svn_revision(&mut b, 1, "add files");
    svn_add_dir(&mut b, "trunk", None);
    let body = vec![b'x'; 256];
    for i in 0..n {
        svn_add_file(&mut b, &format!("trunk/f{i}.txt"), &body);
    }
    b
}

// ---- CVS ,v generator ----------------------------------------------------------------------------

fn cvs_single_rev(log: &str, content: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

// ---- output parsing ------------------------------------------------------------------------------

fn parse(line: &str) -> std::collections::BTreeMap<String, String> {
    line.split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
