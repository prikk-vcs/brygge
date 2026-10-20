//! RFC 014 D-4 (batch R): real-world corpus validation. Reads `tools/corpus/real.toml`, fetches each
//! approved repository into a cache directory (never committed, never re-fetched once its pin/hash
//! verifies), decodes it with every variant its kind supports, runs the real `brygge` binary's
//! `verify` and `verify --against-source`, computes a shape summary via `brygge-ir`'s public reader,
//! and prints one machine-readable line per (entry, variant) — the raw data for "RFC 014 D-4: real
//! repositories" in `tools/bench/README.md`.
//!
//! No new dependency: `real.toml` is parsed by a deliberately tiny subset of TOML (`[[entry]]` tables
//! of `key = "value"` strings, one per line), by hand, below.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use brygge_decode_cvs::{Options as CvsOpts, Source as CvsSource};
use brygge_decode_git::Options as GitOpts;
use brygge_decode_hg::Options as HgOpts;
use brygge_decode_svn::{Options as SvnOpts, Source as SvnSource};
use brygge_ir::Ir;
use brygge_ir::model::PathOp;

use crate::{peak_rss_kib, reset_peak_rss};

#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub url: String,
    pub license: String,
    pub pin: String,
    pub hash: Option<String>,
    pub note: String,
}

/// Parse `real.toml`'s `[[entry]]` tables. Unknown keys are ignored (forward-compatible); a required
/// key missing from an `[[entry]]` panics naming which one, since this is a dev tool run by hand, not a
/// path that needs to recover from a malformed config.
pub fn parse_toml(src: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut cur: Option<std::collections::BTreeMap<String, String>> = None;
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[entry]]" {
            if let Some(fields) = cur.take() {
                entries.push(entry_from(fields));
            }
            cur = Some(std::collections::BTreeMap::new());
            continue;
        }
        if let Some((key, rest)) = line.split_once('=') {
            let key = key.trim();
            let val = rest.trim();
            let val = val
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(val);
            if let Some(fields) = cur.as_mut() {
                fields.insert(key.to_string(), val.to_string());
            }
        }
    }
    if let Some(fields) = cur {
        entries.push(entry_from(fields));
    }
    entries
}

fn entry_from(mut f: std::collections::BTreeMap<String, String>) -> Entry {
    let mut req = |k: &str| {
        f.remove(k)
            .unwrap_or_else(|| panic!("real.toml: entry missing required key `{k}`"))
    };
    Entry {
        kind: req("kind"),
        name: req("name"),
        url: req("url"),
        license: req("license"),
        pin: req("pin"),
        hash: f.remove("hash"),
        note: f.remove("note").unwrap_or_default(),
    }
}

/// Fetch (or reuse) `entry` under `cache_dir/<kind>/<name>`, verify its pin/hash, then decode each
/// variant its kind supports **in its own subprocess** (`real-decode-one`), so the peak RSS printed for
/// each is that one decode's own high-water mark, not a running total across every repository this
/// process has already decoded (the same reason `run_matrix` spawns `run` per scenario, below). `false`
/// overall on any entry's fetch/verify failure or any variant's non-zero exit (reported, not panicked —
/// a flaky network or a moved repository is not a brygge bug).
pub fn run(toml_path: &Path, cache_dir: &Path) -> bool {
    let src = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let entries = parse_toml(&src);
    std::fs::create_dir_all(cache_dir).expect("create the cache directory");
    let exe = std::env::current_exe().expect("current exe");
    let mut ok = true;
    for entry in &entries {
        println!(
            "# {} ({}) — {} — pin {} — {}",
            entry.name, entry.kind, entry.url, entry.pin, entry.license
        );
        if !entry.note.is_empty() {
            println!("#   {}", entry.note);
        }
        match fetch_and_verify(entry, cache_dir) {
            Ok(_path) => {
                for variant in variant_names(&entry.kind) {
                    let out = Command::new(&exe)
                        .args([
                            "real-decode-one",
                            &toml_path.display().to_string(),
                            &cache_dir.display().to_string(),
                            &entry.name,
                            variant,
                        ])
                        .output()
                        .expect("spawn self");
                    print!("{}", String::from_utf8_lossy(&out.stdout));
                    if !out.status.success() {
                        ok = false;
                        eprint!("{}", String::from_utf8_lossy(&out.stderr));
                    }
                }
            }
            Err(why) => {
                ok = false;
                eprintln!("{}: {why}", entry.name);
                println!("name={} variant=fetch exit=fetch_failed", entry.name);
            }
        }
    }
    ok
}

/// One (entry, variant), run by `real-decode-one` in its own freshly spawned process. Re-parses
/// `real.toml` and re-runs `fetch_and_verify` (idempotent: the fetch is skipped when the destination
/// already exists, and the pin/hash check is cheap) so this needs nothing from the parent but the two
/// paths and the names — no shared state to serialize across the process boundary.
pub fn run_one_variant(toml_path: &Path, cache_dir: &Path, name: &str, variant: &str) -> bool {
    let src = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let entries = parse_toml(&src);
    let Some(entry) = entries.iter().find(|e| e.name == name) else {
        eprintln!("real.toml: no entry named {name}");
        return false;
    };
    let path = match fetch_and_verify(entry, cache_dir) {
        Ok(path) => path,
        Err(why) => {
            eprintln!("{}: {why}", entry.name);
            println!("name={} variant=fetch exit=fetch_failed", entry.name);
            return false;
        }
    };
    match decode_one(entry, &path, variant) {
        Ok((ir, time_ms, peak)) => {
            print_row(entry, variant, &path, &ir, time_ms, peak);
            true
        }
        Err(why) => {
            eprintln!("{} [{variant}]: decode failed: {why}", entry.name);
            println!(
                "name={} variant={variant} exit=decode_failed reason={}",
                entry.name,
                why.replace(' ', "_")
            );
            false
        }
    }
}

fn fetch_and_verify(entry: &Entry, cache_dir: &Path) -> Result<PathBuf, String> {
    let dest = cache_dir.join(&entry.kind).join(&entry.name);
    match entry.kind.as_str() {
        "git" => {
            if !dest.join(".git").exists() {
                std::fs::create_dir_all(dest.parent().expect("has a parent")).ok();
                run_cmd(
                    "git",
                    &["clone", "--quiet", &entry.url, &dest.display().to_string()],
                )?;
            }
            run_cmd(
                "git",
                &[
                    "-C",
                    &dest.display().to_string(),
                    "checkout",
                    "--quiet",
                    &entry.pin,
                ],
            )?;
            let head = run_cmd_output(
                "git",
                &["-C", &dest.display().to_string(), "rev-parse", "HEAD"],
            )?;
            if head.trim() != entry.pin {
                return Err(format!(
                    "pin mismatch: HEAD is {} after checking out {}",
                    head.trim(),
                    entry.pin
                ));
            }
            Ok(dest)
        }
        "hg" => {
            if !dest.join(".hg").exists() {
                std::fs::create_dir_all(dest.parent().expect("has a parent")).ok();
                run_cmd(
                    "hg",
                    &["clone", "--quiet", &entry.url, &dest.display().to_string()],
                )?;
            }
            run_cmd(
                "hg",
                &[
                    "-R",
                    &dest.display().to_string(),
                    "update",
                    "--quiet",
                    "-r",
                    &entry.pin,
                ],
            )?;
            let node = run_cmd_output(
                "hg",
                &[
                    "-R",
                    &dest.display().to_string(),
                    "log",
                    "-r",
                    &entry.pin,
                    "--template",
                    "{node}",
                ],
            )?;
            if node.trim() != entry.pin {
                return Err(format!(
                    "pin mismatch: {} resolved to {}",
                    entry.pin,
                    node.trim()
                ));
            }
            Ok(dest)
        }
        "cvs" => {
            std::fs::create_dir_all(&dest).ok();
            run_cmd(
                "rsync",
                &[
                    "-a",
                    "--delete",
                    &entry.url,
                    &format!("{}/", dest.display()),
                ],
            )?;
            let got = tree_sha256(&dest)?;
            let want = entry
                .hash
                .as_deref()
                .ok_or_else(|| "real.toml: cvs entry has no hash".to_string())?;
            if got != want {
                return Err(format!(
                    "tree hash mismatch: fetched {got}, real.toml says {want} (the mirror changed \
                     since this was pinned — refusing to decode stale/unexpected data)"
                ));
            }
            Ok(dest)
        }
        "svn" => {
            // Never fetched automatically (review 066: "cache the dump; never re-fetch"). The dump
            // file must already be in the cache, placed there by hand with the one `svnrdump dump`
            // command the entry's pin names.
            let path = dest.with_extension("dump");
            if !path.exists() {
                return Err(format!(
                    "no cached dump at {} — run: svnrdump dump {} -r 0:{} > {}",
                    path.display(),
                    entry.url,
                    entry.pin,
                    path.display()
                ));
            }
            let got = file_sha256(&path)?;
            let want = entry
                .hash
                .as_deref()
                .ok_or_else(|| "real.toml: svn entry has no hash".to_string())?;
            if got != want {
                return Err(format!(
                    "dump hash mismatch: cached {got}, real.toml says {want}"
                ));
            }
            Ok(path)
        }
        other => Err(format!("unknown kind {other}")),
    }
}

fn run_cmd(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    if !status.success() {
        return Err(format!("{program} {args:?} exited {status}"));
    }
    Ok(())
}

fn run_cmd_output(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} {args:?} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// SHA-256 over the sorted relative paths of every regular file under `dir` and each one's own
/// content — reproducible regardless of mtimes/permissions, and sensitive to any added, removed, or
/// changed file (review 066: the pin for a source with no single revision number).
fn tree_sha256(dir: &Path) -> Result<String, String> {
    let mut paths = Vec::new();
    collect_files(dir, dir, &mut paths)?;
    paths.sort();
    let mut hasher = Sha256::new();
    for rel in paths {
        hasher.update(rel.as_bytes());
        let bytes = std::fs::read(dir.join(&rel)).map_err(|e| format!("read {rel}: {e}"))?;
        hasher.update(sha256_hex(&bytes).as_bytes());
    }
    Ok(hasher.finish_hex())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

// ---- a small, dependency-free SHA-256 (FIPS 180-4) -----------------------------------------------
// Zero new dependencies, matching this crate's own rule. Used only here, for reproducible cache
// verification; not in any shipped crate.

struct Sha256 {
    state: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ],
            buf: Vec::new(),
            len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        while self.buf.len() >= 64 {
            let block: [u8; 64] = self.buf[..64].try_into().expect("64 bytes");
            self.compress(&block);
            self.buf.drain(..64);
        }
    }

    fn finish_hex(mut self) -> String {
        let bit_len = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bit_len.to_be_bytes());
        while self.buf.len() >= 64 {
            let block: [u8; 64] = self.buf[..64].try_into().expect("64 bytes");
            self.compress(&block);
            self.buf.drain(..64);
        }
        self.state.iter().map(|w| format!("{w:08x}")).collect()
    }

    #[allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]
    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(chunk.try_into().expect("4 bytes"));
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finish_hex()
}

// ---- decode, every variant the kind supports -----------------------------------------------------

/// The variant names `decode_one` supports for `kind`, in the order they should run.
fn variant_names(kind: &str) -> &'static [&'static str] {
    match kind {
        "git" => &["default", "infer_renames"],
        "hg" => &["default"],
        "cvs" => &["default", "reconstruct_refs"],
        "svn" => &["default", "reconstruct_refs"],
        _ => &[],
    }
}

/// Decode `entry` at `path` with the named `variant`, timed, with the peak RSS reset immediately
/// before (the caller is expected to be a freshly spawned process, so that peak is this decode's own).
fn decode_one(
    entry: &Entry,
    path: &Path,
    variant: &str,
) -> Result<(Ir, u128, Option<u64>), String> {
    match (entry.kind.as_str(), variant) {
        ("git", "default") => timed(|| {
            brygge_decode_git::decode(path, &GitOpts::default()).map_err(|e| e.to_string())
        }),
        ("git", "infer_renames") => timed(|| {
            let opts = GitOpts {
                infer_renames: true,
                ..GitOpts::default()
            };
            brygge_decode_git::decode(path, &opts).map_err(|e| e.to_string())
        }),
        ("hg", "default") => {
            timed(|| brygge_decode_hg::decode(path, &HgOpts::default()).map_err(|e| e.to_string()))
        }
        ("cvs", "default") => timed(|| {
            brygge_decode_cvs::decode(
                &CvsSource::LocalRepo(path.to_path_buf()),
                &CvsOpts::default(),
            )
            .map_err(|e| e.to_string())
        }),
        ("cvs", "reconstruct_refs") => timed(|| {
            let opts = CvsOpts {
                reconstruct_refs: true,
                ..CvsOpts::default()
            };
            brygge_decode_cvs::decode(&CvsSource::LocalRepo(path.to_path_buf()), &opts)
                .map_err(|e| e.to_string())
        }),
        ("svn", "default") => timed(|| {
            brygge_decode_svn::decode(
                &SvnSource::DumpFile(path.to_path_buf()),
                &SvnOpts::default(),
            )
            .map_err(|e| e.to_string())
        }),
        ("svn", "reconstruct_refs") => timed(|| {
            let opts = SvnOpts {
                reconstruct_refs: true,
                ..SvnOpts::default()
            };
            brygge_decode_svn::decode(&SvnSource::DumpFile(path.to_path_buf()), &opts)
                .map_err(|e| e.to_string())
        }),
        (kind, variant) => Err(format!("unknown kind/variant {kind}/{variant}")),
    }
}

fn timed(f: impl FnOnce() -> Result<Ir, String>) -> Result<(Ir, u128, Option<u64>), String> {
    reset_peak_rss();
    let start = Instant::now();
    let result = f();
    let time_ms = start.elapsed().as_millis();
    let peak = peak_rss_kib();
    result.map(|ir| (ir, time_ms, peak))
}

// ---- shape (the handoff's R-2 §5): atoms, files-per-commit, tree size over depth ------------------

struct Shape {
    atoms: usize,
    files_per_commit: (usize, usize, usize, usize), // min, median, p90, max
    tree_start: usize,
    tree_median: usize,
    tree_max: usize,
    tree_end: usize,
    dropped: usize,
    flagged: usize,
}

/// A single linear pass over `ir.atoms` in their given (canonical) order, applying every atom's ops to
/// one running path set. This is a **summary diagnostic**, not a per-ref reconstruction: atoms on
/// different branches are folded into the same running count, so a repository with many long-lived
/// divergent branches will show a size that is not any one branch's true tree. That is the right
/// trade-off for "how big does this repository get, roughly, as it grows" — the question this shape
/// answers for prikk — not for anything that needs one ref's exact tree (`tree_at`, in the decoders'
/// own tests, does that, atom by atom, and does not scale to this).
fn shape(ir: &Ir) -> Shape {
    let mut live: BTreeSet<String> = BTreeSet::new();
    let mut sizes = Vec::with_capacity(ir.atoms.len());
    let mut per_commit = Vec::with_capacity(ir.atoms.len());
    for atom in &ir.atoms {
        per_commit.push(atom.ops.len());
        for op in &atom.ops {
            match op {
                PathOp::Add { path, .. } => {
                    live.insert(path.clone());
                }
                PathOp::Delete { path, .. } => {
                    live.remove(path);
                }
                PathOp::Modify { .. } | PathOp::Replace { .. } => {}
            }
        }
        sizes.push(live.len());
    }
    let mut sorted_per_commit = per_commit.clone();
    sorted_per_commit.sort_unstable();
    let files_per_commit = percentiles(&sorted_per_commit);
    let (tree_start, tree_end) = (
        sizes.first().copied().unwrap_or(0),
        sizes.last().copied().unwrap_or(0),
    );
    let tree_max = sizes.iter().copied().max().unwrap_or(0);
    let mut sorted_sizes = sizes.clone();
    sorted_sizes.sort_unstable();
    let tree_median = sorted_sizes
        .get(sorted_sizes.len() / 2)
        .copied()
        .unwrap_or(0);
    Shape {
        atoms: ir.atoms.len(),
        files_per_commit,
        tree_start,
        tree_median,
        tree_max,
        tree_end,
        dropped: ir.loss.dropped.len(),
        flagged: ir.flags.len(),
    }
}

fn percentiles(sorted: &[usize]) -> (usize, usize, usize, usize) {
    if sorted.is_empty() {
        return (0, 0, 0, 0);
    }
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let median = sorted[sorted.len() / 2];
    let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
    (min, median, p90, max)
}

// ---- verify, through the real `brygge` binary ------------------------------------------------------

/// The `brygge` binary built alongside this one (same `cargo build`, same `target/<profile>/`).
fn brygge_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("current exe");
    let dir = exe.parent().expect("has a parent");
    let candidate = dir.join("brygge");
    assert!(
        candidate.exists(),
        "expected a `brygge` binary built alongside brygge-bench at {} — build it first \
         (`cargo build --release -p brygge` for a --release brygge-bench, or without --release \
         to match a debug brygge-bench)",
        candidate.display()
    );
    candidate
}

fn print_row(
    entry: &Entry,
    variant: &str,
    source: &Path,
    ir: &Ir,
    time_ms: u128,
    peak_kib: Option<u64>,
) {
    let dir = std::env::temp_dir().join(format!(
        "brygge-bench-real-{}-{}-{}",
        entry.name,
        variant,
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create a scratch dir for the artifact");
    let artifact = dir.join("artifact");
    let artifact_bytes = brygge_ir::artifact::to_bytes(ir);
    let artifact_size = artifact_bytes.len() as u64;
    std::fs::write(&artifact, &artifact_bytes).expect("write the artifact");

    let bin = brygge_bin();
    let verify = run_verify(&bin, &artifact, None);
    let against_source = run_verify(&bin, &artifact, Some(source));

    let shape = shape(ir);
    let content_bytes = ir.content.total_bytes();
    println!(
        "name={} variant={variant} exit=ok time_ms={time_ms} peak_kib={} atoms={} \
         content_bytes={content_bytes} artifact_bytes={artifact_size} \
         files_per_commit_min={} files_per_commit_median={} files_per_commit_p90={} \
         files_per_commit_max={} tree_start={} tree_median={} tree_max={} tree_end={} \
         dropped={} flagged={} verify={verify} against_source={against_source}",
        entry.name,
        peak_kib.map_or_else(|| "n/a".to_string(), |k| k.to_string()),
        shape.atoms,
        shape.files_per_commit.0,
        shape.files_per_commit.1,
        shape.files_per_commit.2,
        shape.files_per_commit.3,
        shape.tree_start,
        shape.tree_median,
        shape.tree_max,
        shape.tree_end,
        shape.dropped,
        shape.flagged,
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn run_verify(bin: &Path, artifact: &Path, against_source: Option<&Path>) -> String {
    let mut cmd = Command::new(bin);
    cmd.arg("verify")
        .arg(artifact)
        .arg("--format")
        .arg("machine");
    if let Some(src) = against_source {
        cmd.arg("--against-source").arg(src);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(out) = cmd.output() else {
        return "could_not_run".to_string();
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        text.push_str(&format!(" exit_code={}", out.status));
    }
    // Keep the machine line compact: the exit code plus whichever `verify.*`/`verify.against_source.*`
    // lines are present, not the whole (possibly long) machine-format dump.
    let mut parts: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("verify") || l.starts_with("exit_code"))
        .collect();
    if parts.is_empty() {
        parts.push("empty");
    }
    parts.join(";")
}
