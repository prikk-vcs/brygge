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
//!   brygge-bench real-fetch <real.toml> <cache-dir>   # fetch whatever real-corpus entry is missing (network)
//!   brygge-bench real-check <real.toml> <cache-dir>   # verify every present entry's pin/hash (no network)
//!   brygge-bench real <real.toml> <cache-dir>         # check (never fetch), decode and measure every entry
//!   brygge-bench ir-equal <a> <b>                     # RFC 021 batch K §5: compare two artifacts' decoded Ir
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
//!   svn-deltas <n>   — the `svn-dump` corpus loaded into a repository and dumped again with `svnadmin dump
//!                      --deltas` (RFC 013 D-2): the same IR from a delta dump (needs `svnadmin`); with
//!                      `svn-dump` it shows what reading svndiff and checking every checksum costs.
//!   svn-checked <n>  — the same, dumped as fulltext by `svnadmin dump`: the dump now states the MD5 and SHA-1 of
//!                      every text, which the decoder checks (RFC 013 D-2). Against a decoder that did not check,
//!                      it is the cost of hashing every node.
//!   svn-dump <n>     — a content-heavy SVN dump of n revisions over 300 files (increment 2: the parsed dump held
//!                      beside the IR).
//!   svn-tags-plain <n>, svn-tags <n> — RFC 020 batch M: a 2,000-file trunk, then n times "edit one trunk
//!                      file, copy trunk to tags/t<n>" (the RFC's own "157" shape). `svn-tags-plain` decodes
//!                      without `--branch-lines`, `svn-tags` with it. Peak should show why a later tag's
//!                      retained snapshot costs the trunk plus every earlier tag (D-1's target).

// A dev-only tool: unwrap/expect/indexing are acceptable here and keep the generators readable.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod corpus;
mod profile;
mod real;

/// P-2 (RFC 022 incident 094, batch W0a): every site in this crate that re-runs `current_exe()` writes
/// this into the child's environment, and refuses to spawn at all if it is already present in its own —
/// so a bench process started as one of these children can never start another of its own. This bounds
/// `brygge-bench`'s own three self-re-run sites (`real::run`, `run_matrix`, `profile::run`) the same way
/// `brygge`'s own future watchdog marker bounds its recursion: none of these three can recurse, however
/// they come to be called.
pub(crate) const BENCH_CHILD_ENV: &str = "BRYGGE_BENCH_CHILD";

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use brygge_decode_cvs::{Options as CvsOpts, Source as CvsSource};
use brygge_decode_git::Options as GitOpts;
use brygge_decode_svn::{Options as SvnOpts, Source as SvnSource};
use brygge_ir::Ir;
use brygge_ir::model::{Content, PathOp};

use corpus::Stats;

const SCENARIOS: &[(&str, &[u64])] = &[
    ("svn-revs", &[1_000, 5_000, 20_000]),
    ("svn-content", &[200, 1_000, 4_000]),
    ("cvs", &[200, 1_000, 4_000]),
    ("git-commits", &[1_000, 5_000, 20_000]),
    ("git-content", &[1_000, 5_000]),
    ("cvs-revs", &[200, 1_000, 5_000]),
    ("svn-dump", &[1_000, 5_000, 20_000]),
    ("svn-deltas", &[1_000, 5_000, 20_000]),
    ("svn-checked", &[1_000, 5_000, 20_000]),
    ("cvs-branches", &[200, 1_000, 5_000]),
    ("cvs-branches-plain", &[200, 1_000, 5_000]),
    // RFC 020 D-1/D-2 (batch S): `max_retained_entries` — which 0.8.1's quadratic retention crossed at
    // N=150 for this K (2,000), the reason this table once stood in with 140 — is gone; a shared tree's
    // retention is O(1) per snapshot, so the full handoff matrix (10, 40, 160, 640, 2,560) now completes.
    // This in-process run is a quick self-checked sanity pass, not the reported figures: review 169 F-2
    // asks for those from the real binary, one child per run (`tools/bench/svn-tags-corpus.py`), not
    // from this harness's own in-process call.
    ("svn-tags-plain", &[10, 40, 160, 640, 2_560]),
    ("svn-tags", &[10, 40, 160, 640, 2_560]),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let scenario = args.get(2).map(String::as_str).unwrap_or("");
            let n: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            run_one(scenario, n);
        }
        Some("real") => {
            let Some(toml_path) = args.get(2).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench real <real.toml> <cache-dir>");
                std::process::exit(2);
            };
            let Some(cache_dir) = args.get(3).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench real <real.toml> <cache-dir>");
                std::process::exit(2);
            };
            if !real::run(&toml_path, &cache_dir) {
                std::process::exit(1);
            }
        }
        // Review 123 F-1: fetching is separate from checking, and from `real`'s own decode/measure.
        // `real-fetch` is the only subcommand that touches the network; it fetches whatever entry is
        // missing and leaves every entry already present untouched. `real-check` verifies every present
        // entry's pin/hash from disk alone, with no network, and reports an absent entry as absent.
        Some("real-fetch") => {
            let Some(toml_path) = args.get(2).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench real-fetch <real.toml> <cache-dir>");
                std::process::exit(2);
            };
            let Some(cache_dir) = args.get(3).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench real-fetch <real.toml> <cache-dir>");
                std::process::exit(2);
            };
            if !real::fetch(&toml_path, &cache_dir) {
                std::process::exit(1);
            }
        }
        Some("real-check") => {
            let Some(toml_path) = args.get(2).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench real-check <real.toml> <cache-dir>");
                std::process::exit(2);
            };
            let Some(cache_dir) = args.get(3).map(PathBuf::from) else {
                eprintln!("usage: brygge-bench real-check <real.toml> <cache-dir>");
                std::process::exit(2);
            };
            if !real::check(&toml_path, &cache_dir) {
                std::process::exit(1);
            }
        }
        // One (entry, variant), in its own process: so its peak RSS reflects that decode alone, the
        // same "one decode per subprocess" method `run_matrix`/`run_one` use for the synthetic
        // scenarios above (the handoff asks for "the bench harness's method"). Spawned by `real::run`,
        // not meant to be typed by hand.
        Some("real-decode-one") => {
            let (Some(toml_path), Some(cache_dir), Some(name), Some(variant)) = (
                args.get(2).map(PathBuf::from),
                args.get(3).map(PathBuf::from),
                args.get(4).cloned(),
                args.get(5).cloned(),
            ) else {
                eprintln!(
                    "usage: brygge-bench real-decode-one <real.toml> <cache-dir> <name> <variant>"
                );
                std::process::exit(2);
            };
            if !real::run_one_variant(&toml_path, &cache_dir, &name, &variant) {
                std::process::exit(1);
            }
        }
        // RFC 015 D-1: where a real decode's peak memory goes, phase by phase (P1..P6), for the real
        // corpus's two largest entries and the largest synthetic scenario per kind.
        Some("profile") => {
            let (Some(out_dir), Some(cache_dir)) = (
                args.get(2).map(PathBuf::from),
                args.get(3).map(PathBuf::from),
            ) else {
                eprintln!("usage: brygge-bench profile <out-dir> <real-cache-dir>");
                std::process::exit(2);
            };
            let synth_dir = out_dir.join("synth");
            std::fs::create_dir_all(&synth_dir).expect("create the synthetic-corpus directory");
            let mut targets = vec![
                profile::Target {
                    label: "fish-shell",
                    source: profile::ProfileSource::Git(cache_dir.join("git").join("fish-shell")),
                },
                profile::Target {
                    label: "tortoisehg-thg",
                    source: profile::ProfileSource::Hg(cache_dir.join("hg").join("tortoisehg-thg")),
                },
            ];
            for (scenario, n) in [
                ("git-commits", 20_000u64),
                ("cvs-revs", 5_000),
                ("svn-dump", 20_000),
            ] {
                let dir = synth_dir.join(scenario);
                std::fs::create_dir_all(&dir).expect("create the scenario's own directory");
                let Some(corpus) = prepare(scenario, n, &dir) else {
                    panic!("unknown scenario {scenario}");
                };
                let source = match corpus.source {
                    Source::Git(p) => profile::ProfileSource::Git(p),
                    Source::Cvs(p) | Source::CvsRefs(p) => profile::ProfileSource::Cvs(p),
                    Source::Svn(p) | Source::SvnBranchLines(p) => profile::ProfileSource::Svn(p),
                };
                targets.push(profile::Target {
                    label: match scenario {
                        "git-commits" => "git-commits-20000",
                        "cvs-revs" => "cvs-revs-5000",
                        "svn-dump" => "svn-dump-20000",
                        other => other,
                    },
                    source,
                });
            }
            // RFC 020 batch M: codeblocks and `svn-tags` (N=140 — see `SCENARIOS`'s comment on why not
            // the handoff's 160), each in both modes, for the memory/time breakdown by phase.
            let codeblocks = cache_dir.join("svn").join("codeblocks.dump");
            targets.push(profile::Target {
                label: "codeblocks",
                source: profile::ProfileSource::Svn(codeblocks.clone()),
            });
            targets.push(profile::Target {
                label: "codeblocks-branch-lines",
                source: profile::ProfileSource::SvnBranchLines(codeblocks),
            });
            let tags_dir = synth_dir.join("svn-tags-140");
            std::fs::create_dir_all(&tags_dir)
                .expect("create the svn-tags scenario's own directory");
            let tags_path = tags_dir.join("in.dump");
            corpus::svn_tags(&tags_path, 140);
            targets.push(profile::Target {
                label: "svn-tags-140",
                source: profile::ProfileSource::Svn(tags_path.clone()),
            });
            targets.push(profile::Target {
                label: "svn-tags-140-branch-lines",
                source: profile::ProfileSource::SvnBranchLines(tags_path),
            });
            profile::run(&targets, &out_dir.join("artifacts"));
        }
        Some("profile-one") => {
            let (Some(phase), Some(kind), Some(path)) = (
                args.get(2).cloned(),
                args.get(3).cloned(),
                args.get(4).map(PathBuf::from),
            ) else {
                eprintln!("usage: brygge-bench profile-one <p1|p2> <kind> <path>");
                std::process::exit(2);
            };
            profile::run_one(&phase, &kind, &path);
        }
        Some("profile-spill") => {
            let (Some(kind), Some(path), Some(spill_dir)) = (
                args.get(2).cloned(),
                args.get(3).map(PathBuf::from),
                args.get(4).map(PathBuf::from),
            ) else {
                eprintln!("usage: brygge-bench profile-spill <kind> <path> <spill-dir>");
                std::process::exit(2);
            };
            profile::run_spill(&kind, &path, &spill_dir);
        }
        Some("ir-equal") => {
            // RFC 021 batch K §5: compares two artifacts' decoded `Ir` values, of any supported
            // contract — the cross-version proof that the same source history gives the same `Ir`
            // (including every atom's `AtomId`) regardless of which contract wrote the artifact.
            let (Some(a_path), Some(b_path)) = (
                args.get(2).map(PathBuf::from),
                args.get(3).map(PathBuf::from),
            ) else {
                eprintln!("usage: brygge-bench ir-equal <a> <b>");
                std::process::exit(2);
            };
            if !ir_equal(&a_path, &b_path) {
                std::process::exit(1);
            }
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
                Source::Svn(p) | Source::SvnBranchLines(p) => ("svn", p),
                Source::Cvs(p) | Source::CvsRefs(p) => ("cvs", p),
                Source::Git(p) => ("git", p),
            };
            println!("{kind} {}", path.display());
        }
        _ => run_matrix(),
    }
}

/// RFC 021 batch K §5: read two artifacts (of any contract this build supports) and compare their
/// decoded `Ir` values — atoms (matched by `source`, never by `AtomId`: the point is to find out
/// whether the ids themselves agree, not to assume they do), refs, the loss record, flags, transforms,
/// and provenance. Each artifact's own contract version is printed, never compared (a 0.3.0 and a
/// 0.4.0 artifact of the same history are expected to differ only in which contract wrote them).
/// Reports the **first** difference found, naming the atom's own source id where the difference is
/// atom-level. Returns `true` when every part is equal.
fn ir_equal(a_path: &Path, b_path: &Path) -> bool {
    let a = match brygge_ir::from_file(a_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}: {e}", a_path.display());
            return false;
        }
    };
    let b = match brygge_ir::from_file(b_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}: {e}", b_path.display());
            return false;
        }
    };
    println!("a: {} (contract {})", a_path.display(), a.contract);
    println!("b: {} (contract {})", b_path.display(), b.contract);

    if a.ir.atoms.len() != b.ir.atoms.len() {
        println!(
            "DIFFER: {} atom(s) in a, {} in b",
            a.ir.atoms.len(),
            b.ir.atoms.len()
        );
        return false;
    }

    let b_by_source: std::collections::HashMap<&brygge_ir::SourceIdentity, &brygge_ir::ChangeAtom> =
        b.ir.atoms.iter().map(|at| (&at.source, at)).collect();
    for a_atom in &a.ir.atoms {
        let Some(&b_atom) = b_by_source.get(&a_atom.source) else {
            println!(
                "DIFFER: atom (source {:?}) in a has no match in b (by source identity)",
                a_atom.source
            );
            return false;
        };
        if a_atom.id != b_atom.id {
            println!(
                "DIFFER: atom (source {:?}): id a={} b={}",
                a_atom.source, a_atom.id, b_atom.id
            );
            return false;
        }
        if a_atom.parents != b_atom.parents {
            println!(
                "DIFFER: atom {} (source {:?}): parents differ",
                a_atom.id, a_atom.source
            );
            return false;
        }
        if a_atom.ops != b_atom.ops {
            println!(
                "DIFFER: atom {} (source {:?}): ops differ",
                a_atom.id, a_atom.source
            );
            return false;
        }
        if a_atom.copies != b_atom.copies {
            println!(
                "DIFFER: atom {} (source {:?}): copies differ",
                a_atom.id, a_atom.source
            );
            return false;
        }
        if a_atom.metadata != b_atom.metadata {
            println!(
                "DIFFER: atom {} (source {:?}): metadata differ",
                a_atom.id, a_atom.source
            );
            return false;
        }
        if a_atom.status != b_atom.status {
            println!(
                "DIFFER: atom {} (source {:?}): status differ",
                a_atom.id, a_atom.source
            );
            return false;
        }
    }

    if a.ir.refs != b.ir.refs {
        println!("DIFFER: refs differ");
        return false;
    }
    if a.ir.loss != b.ir.loss {
        println!("DIFFER: loss boundary differs");
        return false;
    }
    if a.ir.flags != b.ir.flags {
        println!("DIFFER: flags differ");
        return false;
    }
    if a.ir.transforms != b.ir.transforms {
        println!("DIFFER: transforms differ");
        return false;
    }
    // RFC 021 batch G §5: compared field by field, and `params` key by key, so one differing key (for
    // example `floor`, across a batch that changes it) is named on its own — never buried inside a
    // whole-provenance Debug dump that would also hide any *other* difference sitting beside it.
    if a.ir.provenance.source != b.ir.provenance.source {
        println!(
            "DIFFER: provenance.source differs — a={:?} b={:?}",
            a.ir.provenance.source, b.ir.provenance.source
        );
        return false;
    }
    if a.ir.provenance.brygge_version != b.ir.provenance.brygge_version {
        println!(
            "DIFFER: provenance.brygge_version differs — a={:?} b={:?}",
            a.ir.provenance.brygge_version, b.ir.provenance.brygge_version
        );
        return false;
    }
    if a.ir.provenance.decoder != b.ir.provenance.decoder {
        println!(
            "DIFFER: provenance.decoder differs — a={:?} b={:?}",
            a.ir.provenance.decoder, b.ir.provenance.decoder
        );
        return false;
    }
    if a.ir.provenance.decoder_version != b.ir.provenance.decoder_version {
        println!(
            "DIFFER: provenance.decoder_version differs — a={:?} b={:?}",
            a.ir.provenance.decoder_version, b.ir.provenance.decoder_version
        );
        return false;
    }
    let a_keys: std::collections::BTreeSet<&String> = a.ir.provenance.params.keys().collect();
    let b_keys: std::collections::BTreeSet<&String> = b.ir.provenance.params.keys().collect();
    for key in a_keys.union(&b_keys) {
        let av = a.ir.provenance.params.get(*key);
        let bv = b.ir.provenance.params.get(*key);
        if av != bv {
            println!("DIFFER: provenance.params[{key:?}] differs — a={av:?} b={bv:?}");
            return false;
        }
    }

    println!(
        "EQUAL: every atom (including its id), ref, drop, flag, transform and the provenance match."
    );
    true
}

/// P-2 (RFC 022 incident 094): the one point in [`run_matrix`] that re-runs this binary, narrowed to
/// exactly the `Command` construction so that everything around it (`SCENARIOS`, the table printing,
/// the pass/fail bookkeeping) stays identical, and reachable, in both builds. Refuses outright if this
/// process is itself already one of these children (`BENCH_CHILD_ENV` already set) — that should never
/// happen, so a panic here is a bug being caught, not a normal outcome to recover from.
#[cfg(not(test))]
fn spawn_self_run(scenario: &str, n: u64) -> std::process::Output {
    assert!(
        std::env::var_os(BENCH_CHILD_ENV).is_none(),
        "refused: brygge-bench never re-runs itself from inside an already-spawned child (P-2, \
         incident 094)"
    );
    let exe = std::env::current_exe().expect("current exe");
    Command::new(&exe)
        .args(["run", scenario, &n.to_string()])
        .env(BENCH_CHILD_ENV, "1")
        .output()
        .expect("spawn self")
}

/// P-1(c)'s sibling for this crate (P-2, incident 094): under a unit-test build, this contains no
/// [`std::process::Command`] at all, so calling it can never spawn anything, whatever the environment.
#[cfg(test)]
fn spawn_self_run(_scenario: &str, _n: u64) -> std::process::Output {
    panic!(
        "a self re-run is never compiled into a test build (P-2, incident 094); its non-test form \
         guards with {BENCH_CHILD_ENV}"
    );
}

/// Run every (scenario, scale) as a subprocess and print a table. A scenario whose self-check fails (or that
/// crashes) prints `FAILED`, and the run exits non-zero.
fn run_matrix() {
    println!("brygge decoder memory/time harness (RFC 010 OQ-F)");
    println!(
        "{:<14} {:>8} {:>10} {:>9} {:>8} {:>8} {:>12}  check",
        "scenario", "scale", "peak_KiB", "time_ms", "atoms", "blobs", "content_B"
    );
    let mut failed = false;
    for (scenario, scales) in SCENARIOS {
        for &n in *scales {
            let out = spawn_self_run(scenario, n);
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
    /// SVN, decoded with `--branch-lines` (RFC 020 batch M: `svn-tags`'s own mode).
    SvnBranchLines(PathBuf),
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
    let check = self_check(&corpus, &ir, got).and_then(|()| {
        if scenario == "svn-tags" {
            check_svn_tags_refs(n, &ir)
        } else {
            Ok(())
        }
    });
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
        "svn-deltas" => {
            let path = dir.join("in.dump");
            let expected = svn_delta_dump(&path, n, dir, true);
            Corpus {
                source: Source::Svn(path),
                expected: Some(expected),
                cvs_content_check: false,
            }
        }
        "svn-checked" => {
            let path = dir.join("in.dump");
            let expected = svn_delta_dump(&path, n, dir, false);
            Corpus {
                source: Source::Svn(path),
                expected: Some(expected),
                cvs_content_check: false,
            }
        }
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
        "svn-tags-plain" => {
            let path = dir.join("in.dump");
            let expected = corpus::svn_tags(&path, n);
            Corpus {
                source: Source::Svn(path),
                expected: Some(expected),
                cvs_content_check: false,
            }
        }
        "svn-tags" => {
            let path = dir.join("in.dump");
            // The generator's own Stats (atoms/blobs/bytes) describe the *plain* decode; `--branch-lines`
            // partitions the same revisions into one line per root, so this scenario's self-check is the
            // dedicated `check_svn_tags_refs` below (ref count), not the generic `Stats` comparison —
            // `expected: None` here deliberately skips the atoms/blobs/bytes check for this one scenario.
            let _ = corpus::svn_tags(&path, n);
            Corpus {
                source: Source::SvnBranchLines(path),
                expected: None,
                cvs_content_check: false,
            }
        }
        _ => return None,
    })
}

/// The `svn-dump` corpus as a **real** `svnadmin` dump (which states every text's checksums): generated as a
/// fulltext dump, loaded into a scratch repository with `svnadmin load`, and dumped again into `path`, with
/// `--deltas` or as fulltext. Returns the IR statistics of the generator (either form must give the same IR).
fn svn_delta_dump(path: &Path, n: u64, dir: &Path, deltas: bool) -> Stats {
    let full = dir.join("full.dump");
    let expected = corpus::svn_dump(&full, n);
    let repo = dir.join("repo");
    run_svnadmin(&["create", &repo.display().to_string()], None, None);
    run_svnadmin(
        &["load", "--quiet", &repo.display().to_string()],
        Some(&full),
        None,
    );
    let repo_arg = repo.display().to_string();
    let mut dump_args = vec!["dump", "--quiet"];
    if deltas {
        dump_args.push("--deltas");
    }
    dump_args.push(&repo_arg);
    run_svnadmin(&dump_args, None, Some(path));
    let _ = std::fs::remove_file(&full);
    let _ = std::fs::remove_dir_all(&repo);
    expected
}

fn run_svnadmin(args: &[&str], stdin: Option<&Path>, stdout: Option<&Path>) {
    let mut cmd = Command::new("svnadmin");
    cmd.args(args);
    if let Some(p) = stdin {
        cmd.stdin(std::fs::File::open(p).expect("open the dump to load"));
    }
    if let Some(p) = stdout {
        cmd.stdout(std::fs::File::create(p).expect("create the delta dump"));
    }
    let status = cmd.status().expect("run svnadmin (needed for svn-deltas)");
    assert!(status.success(), "svnadmin {args:?} failed");
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
        Source::SvnBranchLines(path) => {
            let opts = SvnOpts {
                branch_lines: true,
                ..SvnOpts::default()
            };
            brygge_decode_svn::decode(&SvnSource::DumpFile(path.clone()), &opts)
                .expect("svn decode (branch-lines)")
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
            PathOp::Add { path, content, .. } | PathOp::Modify { path, content, .. } => {
                match content {
                    Content::Blob { blob, .. } => (path, blob),
                    Content::Submodule { .. } => {
                        return Err(format!("atom {idx}: unexpected submodule"));
                    }
                }
            }
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
            .map_err(|e| format!("atom {idx}: {e}"))?
            .ok_or_else(|| format!("atom {idx}: blob missing"))?;
        if got != want {
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

/// `svn-tags` (`--branch-lines`): one ref per root — the trunk, plus one per tag — so `n` tags give `n + 1`
/// refs exactly, regardless of how the per-root atom count itself splits.
fn check_svn_tags_refs(n: u64, ir: &Ir) -> Result<(), String> {
    let want = usize::try_from(n + 1).expect("n fits");
    let got = ir.refs.len();
    if got != want {
        return Err(format!("{got} refs, expected {want} (trunk + {n} tags)"));
    }
    Ok(())
}

// ---- peak RSS (Linux /proc, no unsafe) -----------------------------------------------------------

pub(crate) fn peak_rss_kib() -> Option<u64> {
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
pub(crate) fn reset_peak_rss() {
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

// ---- P-2 tests (RFC 022 incident 094) -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::spawn_self_run;

    #[test]
    #[should_panic(expected = "a self re-run is never compiled into a test build")]
    fn run_matrix_never_spawns_in_a_unit_test_build() {
        spawn_self_run("svn-revs", 1);
    }
}
