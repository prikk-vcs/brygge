//! RFC 014 D-4 (batch R): real-world corpus validation. Reads `tools/corpus/real.toml`, decodes each
//! approved repository (already fetched into a cache directory — never here) with every variant its
//! kind supports, runs the real `brygge` binary's `verify` and `verify --against-source`, computes a
//! shape summary via `brygge-ir`'s public reader, and prints one machine-readable line per (entry,
//! variant) — the raw data for "RFC 014 D-4: real repositories" in `tools/bench/README.md`.
//!
//! **Fetching and checking are separate** (review 123 F-1), so that neither `real` (which decodes and
//! measures) nor `tools/fixtures.sh verify` (which must touch no network) ever fetches by accident:
//! - [`fetch`] fetches whatever entry is missing, into a temporary sibling of its final path, verifies
//!   it there, and only then renames it into place — never touching an entry already present. This is
//!   the only function in this module that uses the network.
//! - [`check`] (and `real`/`real-decode-one`'s own use of [`check_entry`]) verifies every present
//!   entry's pin/hash from what is already on disk, with no network at all, and reports an absent entry
//!   as absent rather than fetching it.
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
    /// Git only (review 128 G-1): the SHA-256 of `git for-each-ref --format='%(objectname) %(refname)'`
    /// after `fetch_entry` normalizes the clone's refs to exactly the pin's history — so a tag added
    /// later to an old commit, or a moved branch, is refused loudly instead of drifting silently into
    /// what the corpus decodes.
    pub refs: Option<String>,
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
        refs: f.remove("refs"),
        note: f.remove("note").unwrap_or_default(),
    }
}

/// What `fetch_entry` did.
pub enum FetchOutcome {
    /// The destination already existed; nothing was fetched or touched (review 123 F-1: fetching once
    /// means never re-syncing an entry that is already there).
    AlreadyPresent,
    Fetched,
}

/// Why `check_entry` refused an entry. Distinct from a mismatch: an absent entry is not yet an error by
/// itself (the caller decides — `check` reports it and keeps going; `run`/`run_one_variant` refuse with
/// "run `tools/fixtures.sh fetch`").
#[derive(Debug)]
pub enum CheckError {
    Absent,
    Mismatch(String),
}

/// Fetch every entry in `toml_path` that is not already present under `cache_dir`, verify its pin/hash
/// before it is ever visible at its final path, and leave every already-present entry untouched —
/// **no measurement, no decode, no network for anything already cached** (review 123 F-1). `false` if
/// any entry's fetch failed.
pub fn fetch(toml_path: &Path, cache_dir: &Path) -> bool {
    let src = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let entries = parse_toml(&src);
    std::fs::create_dir_all(cache_dir).expect("create the cache directory");
    let mut ok = true;
    for entry in &entries {
        match fetch_entry(entry, cache_dir) {
            Ok(FetchOutcome::AlreadyPresent) => {
                println!("{} ({}): already present", entry.name, entry.kind);
            }
            Ok(FetchOutcome::Fetched) => {
                println!("{} ({}): fetched and verified", entry.name, entry.kind);
            }
            Err(why) => {
                ok = false;
                eprintln!("{} ({}): fetch failed: {why}", entry.name, entry.kind);
            }
        }
    }
    ok
}

/// Check every entry in `toml_path` against what is present under `cache_dir`, with **no network and no
/// fetch** (review 123 F-1): an absent entry is reported as absent, a present one whose pin/hash does not
/// match is reported as a mismatch naming the entry, and the function returns `false` if anything is
/// absent or mismatched.
pub fn check(toml_path: &Path, cache_dir: &Path) -> bool {
    let src = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let entries = parse_toml(&src);
    let mut ok = true;
    for entry in &entries {
        match check_entry(entry, cache_dir) {
            Ok(_path) => println!("{} ({}): ok", entry.name, entry.kind),
            Err(CheckError::Absent) => {
                ok = false;
                println!("{} ({}): absent", entry.name, entry.kind);
            }
            Err(CheckError::Mismatch(why)) => {
                ok = false;
                eprintln!("{} ({}): {why}", entry.name, entry.kind);
            }
        }
    }
    ok
}

/// P-2 (RFC 022 incident 094): the one point in [`run`] that re-runs this binary, narrowed to exactly
/// the `Command` construction so that everything around it (parsing `real.toml`, the per-entry
/// printing, the pass/fail bookkeeping) stays identical, and reachable, in both builds. Refuses outright
/// if this process is itself already one of these children (`BENCH_CHILD_ENV` already set) — that
/// should never happen, so a panic here is a bug being caught, not a normal outcome to recover from.
#[cfg(not(test))]
fn spawn_self_decode_one(
    toml_path: &Path,
    cache_dir: &Path,
    name: &str,
    variant: &str,
) -> std::process::Output {
    assert!(
        std::env::var_os(crate::BENCH_CHILD_ENV).is_none(),
        "refused: brygge-bench never re-runs itself from inside an already-spawned child (P-2, \
         incident 094)"
    );
    let exe = std::env::current_exe().expect("current exe");
    Command::new(&exe)
        .args([
            "real-decode-one",
            &toml_path.display().to_string(),
            &cache_dir.display().to_string(),
            name,
            variant,
        ])
        .env(crate::BENCH_CHILD_ENV, "1")
        .output()
        .expect("spawn self")
}

/// P-1(c)'s sibling for this crate (P-2, incident 094): under a unit-test build, this contains no
/// [`std::process::Command`] at all, so calling it can never spawn anything, whatever the environment.
#[cfg(test)]
fn spawn_self_decode_one(
    _toml_path: &Path,
    _cache_dir: &Path,
    _name: &str,
    _variant: &str,
) -> std::process::Output {
    panic!("a self re-run is never compiled into a test build (P-2, incident 094)");
}

/// Check (never fetch: review 123 F-1) `entry` under `cache_dir/<kind>/<name>`, then decode each variant
/// its kind supports **in its own subprocess** (`real-decode-one`), so the peak RSS printed for each is
/// that one decode's own high-water mark, not a running total across every repository this process has
/// already decoded (the same reason `run_matrix` spawns `run` per scenario, below). `false` overall on
/// any entry's check failure or any variant's non-zero exit (reported, not panicked — a moved repository
/// or a stale cache is not a brygge bug).
pub fn run(toml_path: &Path, cache_dir: &Path) -> bool {
    let src = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let entries = parse_toml(&src);
    let mut ok = true;
    for entry in &entries {
        println!(
            "# {} ({}) — {} — pin {} — {}",
            entry.name, entry.kind, entry.url, entry.pin, entry.license
        );
        if !entry.note.is_empty() {
            println!("#   {}", entry.note);
        }
        match check_entry(entry, cache_dir) {
            Ok(_path) => {
                for variant in variant_names(&entry.kind) {
                    let out = spawn_self_decode_one(toml_path, cache_dir, &entry.name, variant);
                    print!("{}", String::from_utf8_lossy(&out.stdout));
                    if !out.status.success() {
                        ok = false;
                        eprint!("{}", String::from_utf8_lossy(&out.stderr));
                    }
                }
            }
            Err(CheckError::Absent) => {
                ok = false;
                eprintln!("{}: absent — run `tools/fixtures.sh fetch`", entry.name);
                println!("name={} variant=fetch exit=fetch_failed", entry.name);
            }
            Err(CheckError::Mismatch(why)) => {
                ok = false;
                eprintln!("{}: {why}", entry.name);
                println!("name={} variant=fetch exit=fetch_failed", entry.name);
            }
        }
    }
    ok
}

/// One (entry, variant), run by `real-decode-one` in its own freshly spawned process. Re-parses
/// `real.toml` and re-runs `check_entry` (cheap: local only, no network) so this needs nothing from the
/// parent but the two paths and the names — no shared state to serialize across the process boundary.
pub fn run_one_variant(toml_path: &Path, cache_dir: &Path, name: &str, variant: &str) -> bool {
    let src = std::fs::read_to_string(toml_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", toml_path.display()));
    let entries = parse_toml(&src);
    let Some(entry) = entries.iter().find(|e| e.name == name) else {
        eprintln!("real.toml: no entry named {name}");
        return false;
    };
    let path = match check_entry(entry, cache_dir) {
        Ok(path) => path,
        Err(CheckError::Absent) => {
            eprintln!("{}: absent — run `tools/fixtures.sh fetch`", entry.name);
            println!("name={} variant=fetch exit=fetch_failed", entry.name);
            return false;
        }
        Err(CheckError::Mismatch(why)) => {
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

/// `cache_dir/<kind>/<name>.partial`, or `<dump-path>.partial` for SVN: a sibling of the final
/// destination, in the same directory, so the final `rename` is a same-filesystem move — never a
/// half-written entry at the real path (review 123 F-1).
fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().expect("has a file name").to_os_string();
    name.push(".partial");
    dest.with_file_name(name)
}

fn remove_path(p: &Path) {
    if p.is_dir() {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

fn cvs_present(dest: &Path) -> bool {
    dest.is_dir()
        && std::fs::read_dir(dest)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
}

/// Point `repo`'s own default branch at `pin`, remove every remote (and so every `refs/remotes/*`), and
/// delete every tag not reachable from `pin` (review 128 G-1): a Git pin fixes only the checked-out
/// commit by itself, but the decoder also carries every `refs/heads/*`/`refs/tags/*` it finds, so a
/// clone made later — with upstream's branch moved on and a new tag added — decodes more than the pin.
fn normalize_git_refs(repo: &str, branch: &str, pin: &str) -> Result<(), String> {
    run_cmd(
        "git",
        &[
            "-C",
            repo,
            "update-ref",
            &format!("refs/heads/{branch}"),
            pin,
        ],
    )?;
    let remotes = run_cmd_output("git", &["-C", repo, "remote"])?;
    for name in remotes.lines().filter(|l| !l.is_empty()) {
        run_cmd("git", &["-C", repo, "remote", "remove", name])?;
    }
    let unmerged = run_cmd_output("git", &["-C", repo, "tag", "--no-merged", pin])?;
    for tag in unmerged.lines().filter(|l| !l.is_empty()) {
        run_cmd("git", &["-C", repo, "tag", "-d", tag])?;
    }
    Ok(())
}

/// The SHA-256 of `git for-each-ref`'s own output: every remaining ref's object id and name, in
/// `for-each-ref`'s own (lexical, deterministic) order. After [`normalize_git_refs`], this is exactly
/// the pin's branch and the tags merged into it — nothing a later upstream change could silently move.
fn git_refs_hash(repo: &str) -> Result<String, String> {
    let out = run_cmd_output(
        "git",
        &[
            "-C",
            repo,
            "for-each-ref",
            "--format=%(objectname) %(refname)",
        ],
    )?;
    Ok(sha256_hex(out.as_bytes()))
}

fn delete_hg_bookmarks(repo: &str) -> Result<(), String> {
    let out = run_cmd_output(
        "hg",
        &["-R", repo, "bookmarks", "--template", "{bookmark}\n"],
    )?;
    for name in out.lines().filter(|l| !l.is_empty()) {
        run_cmd("hg", &["-R", repo, "bookmark", "--delete", name])?;
    }
    Ok(())
}

/// `heads(all())` is the revset for every head of every branch, not just the current one (review 128
/// G-1): `hg clone -r <pin>` already pulls only the pin's ancestors, so this should already hold, but a
/// cache someone hand-edited, or a clone of a repository with an unexpected branch structure, must be
/// caught, not assumed.
fn check_hg_single_head(repo: &str, pin: &str) -> Result<(), String> {
    let out = run_cmd_output(
        "hg",
        &[
            "-R",
            repo,
            "log",
            "-r",
            "heads(all())",
            "--template",
            "{node}\n",
        ],
    )?;
    let heads: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    if heads != [pin] {
        return Err(format!(
            "heads(all()) is {heads:?}, expected exactly [{pin}]"
        ));
    }
    Ok(())
}

fn check_hg_no_bookmarks(repo: &str) -> Result<(), String> {
    let out = run_cmd_output(
        "hg",
        &["-R", repo, "bookmarks", "--template", "{bookmark}\n"],
    )?;
    if out.lines().any(|l| !l.is_empty()) {
        return Err(format!("bookmarks present: {}", out.trim()));
    }
    Ok(())
}

/// Fetch `entry` into a temporary sibling of its final destination, verify its pin/hash there, and only
/// then rename it into place — so a reader never sees a partially-fetched entry, and a failed fetch
/// leaves nothing behind at either path (review 123 F-1). Never touches an entry that is already
/// present: this is the only place network is used in this module.
fn fetch_entry(entry: &Entry, cache_dir: &Path) -> Result<FetchOutcome, String> {
    let dest = cache_dir.join(&entry.kind).join(&entry.name);
    match entry.kind.as_str() {
        "git" => {
            if dest.join(".git").exists() {
                return Ok(FetchOutcome::AlreadyPresent);
            }
            let partial = partial_path(&dest);
            remove_path(&partial);
            std::fs::create_dir_all(partial.parent().expect("has a parent")).ok();
            let result = (|| -> Result<(), String> {
                let p = partial.display().to_string();
                run_cmd("git", &["clone", "--quiet", &entry.url, &p])?;
                // The branch the clone itself created (tracking the remote's default branch), captured
                // before `checkout` detaches HEAD at the pin — review 128 G-1 pins this branch at the
                // pin, not wherever upstream's default branch has since moved to.
                let branch = run_cmd_output("git", &["-C", &p, "symbolic-ref", "--short", "HEAD"])?
                    .trim()
                    .to_string();
                run_cmd("git", &["-C", &p, "checkout", "--quiet", &entry.pin])?;
                let head = run_cmd_output("git", &["-C", &p, "rev-parse", "HEAD"])?;
                if head.trim() != entry.pin {
                    return Err(format!(
                        "pin mismatch: HEAD is {} after checking out {}",
                        head.trim(),
                        entry.pin
                    ));
                }
                normalize_git_refs(&p, &branch, &entry.pin)?;
                let got = git_refs_hash(&p)?;
                let want = entry
                    .refs
                    .as_deref()
                    .ok_or_else(|| "real.toml: git entry has no refs hash".to_string())?;
                if got != want {
                    return Err(format!(
                        "refs hash mismatch after pinning: fetched {got}, real.toml says {want} (a \
                         tag or branch upstream has since moved — refusing to use stale/unexpected refs)"
                    ));
                }
                Ok(())
            })();
            if let Err(e) = result {
                remove_path(&partial);
                return Err(e);
            }
            std::fs::rename(&partial, &dest).map_err(|e| format!("rename into place: {e}"))?;
            Ok(FetchOutcome::Fetched)
        }
        "hg" => {
            if dest.join(".hg").exists() {
                return Ok(FetchOutcome::AlreadyPresent);
            }
            let partial = partial_path(&dest);
            remove_path(&partial);
            std::fs::create_dir_all(partial.parent().expect("has a parent")).ok();
            let result = (|| -> Result<(), String> {
                let p = partial.display().to_string();
                // `-r <pin>` pulls only the pin's ancestors (review 128 G-1): a later upstream commit on
                // the same branch never reaches this clone at all, so there is no later history to trim.
                run_cmd(
                    "hg",
                    &["clone", "--quiet", "-r", &entry.pin, &entry.url, &p],
                )?;
                let node = run_cmd_output(
                    "hg",
                    &["-R", &p, "log", "-r", &entry.pin, "--template", "{node}"],
                )?;
                if node.trim() != entry.pin {
                    return Err(format!(
                        "pin mismatch: {} resolved to {}",
                        entry.pin,
                        node.trim()
                    ));
                }
                delete_hg_bookmarks(&p)?;
                check_hg_single_head(&p, &entry.pin)?;
                Ok(())
            })();
            if let Err(e) = result {
                remove_path(&partial);
                return Err(e);
            }
            std::fs::rename(&partial, &dest).map_err(|e| format!("rename into place: {e}"))?;
            Ok(FetchOutcome::Fetched)
        }
        "cvs" => {
            if cvs_present(&dest) {
                return Ok(FetchOutcome::AlreadyPresent);
            }
            let partial = partial_path(&dest);
            remove_path(&partial);
            std::fs::create_dir_all(&partial).ok();
            let result = (|| -> Result<(), String> {
                run_cmd(
                    "rsync",
                    &[
                        "-a",
                        "--delete",
                        &entry.url,
                        &format!("{}/", partial.display()),
                    ],
                )?;
                let got = tree_sha256(&partial)?;
                let want = entry
                    .hash
                    .as_deref()
                    .ok_or_else(|| "real.toml: cvs entry has no hash".to_string())?;
                if got != want {
                    return Err(format!(
                        "tree hash mismatch: fetched {got}, real.toml says {want} (the mirror \
                         changed since this was pinned — refusing to use stale/unexpected data)"
                    ));
                }
                Ok(())
            })();
            if let Err(e) = result {
                remove_path(&partial);
                return Err(e);
            }
            std::fs::rename(&partial, &dest).map_err(|e| format!("rename into place: {e}"))?;
            Ok(FetchOutcome::Fetched)
        }
        "svn" => {
            let path = dest.with_extension("dump");
            if path.exists() {
                return Ok(FetchOutcome::AlreadyPresent);
            }
            let partial = partial_path(&path);
            remove_path(&partial);
            std::fs::create_dir_all(partial.parent().expect("has a parent")).ok();
            let result = (|| -> Result<(), String> {
                let file = std::fs::File::create(&partial)
                    .map_err(|e| format!("create {}: {e}", partial.display()))?;
                let status = Command::new("svnrdump")
                    .args(["dump", &entry.url, "-r", &format!("0:{}", entry.pin)])
                    .stdout(file)
                    .status()
                    .map_err(|e| format!("spawn svnrdump: {e}"))?;
                if !status.success() {
                    return Err(format!("svnrdump dump exited {status}"));
                }
                // Review 128 S-3: `svnrdump` orders a revision's node records by an internal hash that
                // changes from run to run, even for the same content — so the pin hashes a canonical
                // form (each revision's nodes stable-sorted by `Node-path`), not the file as fetched.
                let bytes = std::fs::read(&partial)
                    .map_err(|e| format!("read {}: {e}", partial.display()))?;
                let got = canonical_svn_dump_hash(&bytes)?;
                let want = entry
                    .hash
                    .as_deref()
                    .ok_or_else(|| "real.toml: svn entry has no hash".to_string())?;
                if got != want {
                    return Err(format!(
                        "canonical dump hash mismatch: fetched {got}, real.toml says {want}"
                    ));
                }
                Ok(())
            })();
            if let Err(e) = result {
                remove_path(&partial);
                return Err(e);
            }
            std::fs::rename(&partial, &path).map_err(|e| format!("rename into place: {e}"))?;
            Ok(FetchOutcome::Fetched)
        }
        other => Err(format!("unknown kind {other}")),
    }
}

/// Check `entry` against what is present under `cache_dir`, with **no network**: a git/hg entry's pin is
/// resolved against its already-cloned local history only; a cvs/svn entry's hash is recomputed from the
/// bytes already on disk. `Err(CheckError::Absent)` when nothing is cached yet; `Err(Mismatch)` names
/// what disagrees.
fn check_entry(entry: &Entry, cache_dir: &Path) -> Result<PathBuf, CheckError> {
    let dest = cache_dir.join(&entry.kind).join(&entry.name);
    match entry.kind.as_str() {
        "git" => {
            if !dest.join(".git").exists() {
                return Err(CheckError::Absent);
            }
            let d = dest.display().to_string();
            let head = run_cmd_output("git", &["-C", &d, "rev-parse", "HEAD"])
                .map_err(CheckError::Mismatch)?;
            if head.trim() != entry.pin {
                return Err(CheckError::Mismatch(format!(
                    "HEAD is {}, real.toml pins {}",
                    head.trim(),
                    entry.pin
                )));
            }
            // Review 128 G-1: HEAD alone pins only the checked-out commit, not what a clone's other
            // refs (every `refs/heads/*`/`refs/tags/*` the decoder also carries) point at — so a tag
            // added later to this same commit, or a moved branch, must be caught here too.
            let got = git_refs_hash(&d).map_err(CheckError::Mismatch)?;
            let want = entry.refs.as_deref().unwrap_or_default();
            if got != want {
                return Err(CheckError::Mismatch(format!(
                    "refs hash mismatch: have {got}, real.toml says {want} (a tag or branch moved \
                     since the pin)"
                )));
            }
            Ok(dest)
        }
        "hg" => {
            if !dest.join(".hg").exists() {
                return Err(CheckError::Absent);
            }
            let d = dest.display().to_string();
            let node = run_cmd_output(
                "hg",
                &["-R", &d, "log", "-r", &entry.pin, "--template", "{node}"],
            )
            .map_err(CheckError::Mismatch)?;
            if node.trim() != entry.pin {
                return Err(CheckError::Mismatch(format!(
                    "{} resolved to {}",
                    entry.pin,
                    node.trim()
                )));
            }
            check_hg_single_head(&d, &entry.pin).map_err(CheckError::Mismatch)?;
            check_hg_no_bookmarks(&d).map_err(CheckError::Mismatch)?;
            Ok(dest)
        }
        "cvs" => {
            if !cvs_present(&dest) {
                return Err(CheckError::Absent);
            }
            let got = tree_sha256(&dest).map_err(CheckError::Mismatch)?;
            let want = entry.hash.as_deref().unwrap_or_default();
            if got != want {
                return Err(CheckError::Mismatch(format!(
                    "tree hash mismatch: have {got}, real.toml says {want}"
                )));
            }
            Ok(dest)
        }
        "svn" => {
            let path = dest.with_extension("dump");
            if !path.exists() {
                return Err(CheckError::Absent);
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| CheckError::Mismatch(format!("read {}: {e}", path.display())))?;
            let got = canonical_svn_dump_hash(&bytes).map_err(CheckError::Mismatch)?;
            let want = entry.hash.as_deref().unwrap_or_default();
            if got != want {
                return Err(CheckError::Mismatch(format!(
                    "canonical dump hash mismatch: have {got}, real.toml says {want}"
                )));
            }
            Ok(path)
        }
        other => Err(CheckError::Mismatch(format!("unknown kind {other}"))),
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

/// A minimal SVN dump reader, independent of `brygge-decode-svn`'s own parser (review 128 S-3: the pin
/// must check the input independently of the code it measures) — just enough to split the dump into its
/// records by each one's own stated `Content-length`, without interpreting what a record means beyond
/// whether it names a `Node-path` (so its containing revision's node records can be stable-sorted).
///
/// Format (the same convention this module's own test generator, and real `svnadmin`/`svnrdump` output,
/// both follow): each record is header lines (`Key: Value`) up to a blank line, then exactly
/// `Content-length` body bytes if that header is present, then zero or more blank-line separators before
/// the next record or the end of the file. A record's span runs from its first header byte through
/// those trailing separators, so reordering records never drops or duplicates a byte.
/// A dump record's byte span and, for a node record, the `Node-path` it names.
type SvnDumpRecord = (std::ops::Range<usize>, Option<String>);

fn parse_svn_dump_records(bytes: &[u8]) -> Result<Vec<SvnDumpRecord>, String> {
    let mut records = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let start = pos;
        let mut node_path = None;
        let mut content_length = None;
        loop {
            let nl = bytes[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .ok_or_else(|| "svn dump: unterminated header line".to_string())?;
            let line = &bytes[pos..pos + nl];
            pos += nl + 1;
            if line.is_empty() {
                break;
            }
            let line = std::str::from_utf8(line)
                .map_err(|e| format!("svn dump: a header line is not UTF-8: {e}"))?;
            if let Some((key, value)) = line.split_once(':') {
                let (key, value) = (key.trim(), value.trim());
                if key == "Node-path" {
                    node_path = Some(value.to_string());
                } else if key == "Content-length" {
                    content_length = Some(
                        value
                            .parse::<usize>()
                            .map_err(|e| format!("svn dump: bad Content-length {value:?}: {e}"))?,
                    );
                }
            }
        }
        if let Some(len) = content_length {
            pos = pos
                .checked_add(len)
                .filter(|&p| p <= bytes.len())
                .ok_or_else(|| {
                    "svn dump: Content-length runs past the end of the file".to_string()
                })?;
        }
        while pos < bytes.len() && bytes[pos] == b'\n' {
            pos += 1;
        }
        records.push((start..pos, node_path));
    }
    Ok(records)
}

/// The SHA-256 of `bytes` with each revision's node records **stable-sorted by `Node-path`** (review
/// 128 S-3): `svnrdump`'s own node order within one revision is not stable across runs (it hashes paths
/// internally), even though the history it describes — the multiset of paths and their content, per
/// revision — is identical. A stable sort keeps two records that share a path in their original order,
/// so a replace (delete, then add) is never reordered into an add followed by a delete.
fn canonical_svn_dump_hash(bytes: &[u8]) -> Result<String, String> {
    let records = parse_svn_dump_records(bytes)?;
    let mut canonical: Vec<std::ops::Range<usize>> = Vec::with_capacity(records.len());
    let mut i = 0;
    while i < records.len() {
        let (range, node_path) = &records[i];
        if node_path.is_some() {
            // A node record with no preceding revision/preamble record in this dump: pass it through
            // as-is rather than silently reordering something this parser does not expect to see.
            canonical.push(range.clone());
            i += 1;
            continue;
        }
        canonical.push(range.clone());
        i += 1;
        let group_start = i;
        while i < records.len() && records[i].1.is_some() {
            i += 1;
        }
        let mut group: Vec<usize> = (group_start..i).collect();
        group.sort_by(|&a, &b| records[a].1.cmp(&records[b].1)); // stable: ties keep their order
        for idx in group {
            canonical.push(records[idx].0.clone());
        }
    }
    let mut hasher = Sha256::new();
    for range in canonical {
        hasher.update(&bytes[range]);
    }
    Ok(hasher.finish_hex())
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
pub(crate) fn brygge_bin() -> PathBuf {
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

#[cfg(test)]
mod tests {
    //! Review 123 F-1's level-2 tests: a local Git repository as the entry's URL, a local directory as
    //! the CVS `rsync` source, and a real local SVN repository (`svnrdump` works against a `file://`
    //! URL with no network, the same as `svnadmin dump` would). No real network is used anywhere here.
    use super::*;

    // P-2 (RFC 022 incident 094).
    #[test]
    #[should_panic(expected = "a self re-run is never compiled into a test build")]
    fn run_never_spawns_in_a_unit_test_build() {
        spawn_self_decode_one(
            Path::new("/nonexistent.toml"),
            Path::new("/nonexistent"),
            "entry",
            "default",
        );
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "brygge-bench-real-test-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_cmd(args: &[&str], cwd: &Path) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Review 128 G-1: a Git pin must fix the *decoded refs*, not only `HEAD`. The source repository
    /// here has a commit and a tag past the pin, and a second branch — exactly the shape that let
    /// fish-shell's and jq's corpus figures silently drift with upstream before this fix.
    #[test]
    fn git_entry_fetch_check_corruption_and_no_network() {
        let root = scratch("git");
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        git_cmd(&["init", "--quiet"], &source);
        git_cmd(&["config", "user.email", "t@example.com"], &source);
        git_cmd(&["config", "user.name", "t"], &source);
        std::fs::write(source.join("f.txt"), "hello\n").unwrap();
        git_cmd(&["add", "f.txt"], &source);
        git_cmd(&["commit", "--quiet", "-m", "init"], &source);
        let pin = git_cmd(&["rev-parse", "HEAD"], &source);
        // Whatever this machine's default branch name is (`init.defaultBranch`) — never hardcoded, so
        // the test holds regardless of local git config.
        let default_branch = git_cmd(&["branch", "--show-current"], &source);
        // A tag at the pin: reachable from it, so it must survive normalization. `update-ref` writes a
        // lightweight tag directly, without going through `git tag`'s own porcelain (which this
        // machine's git config turns into an annotated tag requiring a message).
        git_cmd(&["update-ref", "refs/tags/v-old", &pin], &source);
        // A second branch at the pin, with its own commit: a plain clone only ever makes this a
        // remote-tracking ref (refs/remotes/origin/feature), never a local one, so it must leave no
        // trace at all after normalization removes every remote.
        git_cmd(&["branch", "feature"], &source);
        git_cmd(&["checkout", "--quiet", "feature"], &source);
        std::fs::write(source.join("g.txt"), "feature\n").unwrap();
        git_cmd(&["add", "g.txt"], &source);
        git_cmd(&["commit", "--quiet", "-m", "feature work"], &source);
        // Back on the default branch, a commit and a tag past the pin: neither is an ancestor of the
        // pin, so both must be gone after normalization (the branch reset to the pin, the tag deleted
        // as --no-merged).
        git_cmd(&["checkout", "--quiet", &default_branch], &source);
        std::fs::write(source.join("f.txt"), "updated\n").unwrap();
        git_cmd(&["add", "f.txt"], &source);
        git_cmd(&["commit", "--quiet", "-m", "past the pin"], &source);
        let past_pin = git_cmd(&["rev-parse", "HEAD"], &source);
        git_cmd(&["update-ref", "refs/tags/v-new", &past_pin], &source);

        // The refs hash fetch_entry will compute: clone, checkout the pin, normalize, hash — done once
        // here from an independent probe clone, the same way the proof in request 065/068 computed a
        // hash before recording it, so the test does not just assert "whatever fetch_entry happens to
        // produce" against itself.
        let probe = root.join("probe");
        git_cmd(
            &[
                "clone",
                "--quiet",
                &source.display().to_string(),
                &probe.display().to_string(),
            ],
            &root,
        );
        git_cmd(&["checkout", "--quiet", &pin], &probe);
        normalize_git_refs(&probe.display().to_string(), &default_branch, &pin).unwrap();
        let refs_hash = git_refs_hash(&probe.display().to_string()).unwrap();
        std::fs::remove_dir_all(&probe).unwrap();

        let entry = Entry {
            kind: "git".to_string(),
            name: "test-git".to_string(),
            url: source.display().to_string(),
            license: "MIT".to_string(),
            pin: pin.clone(),
            hash: None,
            refs: Some(refs_hash.clone()),
            note: String::new(),
        };
        let cache = root.join("cache");

        // fetch, then check, passes.
        assert!(matches!(
            fetch_entry(&entry, &cache),
            Ok(FetchOutcome::Fetched)
        ));
        assert!(check_entry(&entry, &cache).is_ok());

        let dest = cache.join("git").join("test-git");
        let d = dest.display().to_string();
        // After fetch: only the pin's history and its merged tag remain, and no remote refs.
        let remaining =
            run_cmd_output("git", &["-C", &d, "for-each-ref", "--format=%(refname)"]).unwrap();
        let remaining: Vec<String> = remaining.lines().map(str::to_string).collect();
        assert_eq!(
            remaining,
            [
                format!("refs/heads/{default_branch}"),
                "refs/tags/v-old".to_string()
            ]
        );
        assert!(
            run_cmd_output("git", &["-C", &d, "remote"])
                .unwrap()
                .trim()
                .is_empty(),
            "no remote must survive normalization"
        );

        // a second fetch changes nothing: it must report AlreadyPresent, never re-clone.
        assert!(matches!(
            fetch_entry(&entry, &cache),
            Ok(FetchOutcome::AlreadyPresent)
        ));

        // the check passes with the source deleted: it needs no network.
        std::fs::remove_dir_all(&source).unwrap();
        assert!(check_entry(&entry, &cache).is_ok());

        // moving the cached branch fails the check, naming the pin. `fetch_entry` leaves the clone with
        // a detached HEAD *at* the pin (it checks out the pin by commit, not by branch name), so
        // committing here moves HEAD itself, not `refs/heads/<branch>` — exactly the state a real
        // corrupted cache (someone committed into it) would be in.
        std::fs::write(dest.join("f.txt"), "tampered\n").unwrap();
        git_cmd(&["add", "f.txt"], &dest);
        // A fresh clone carries no identity of its own, unlike `source` above: a CI runner with no
        // global git config (unlike this author's machine) would otherwise fail this commit with
        // "Please tell me who you are."
        git_cmd(&["config", "user.email", "t@example.com"], &dest);
        git_cmd(&["config", "user.name", "t"], &dest);
        git_cmd(&["commit", "--quiet", "-m", "tamper"], &dest);
        match check_entry(&entry, &cache) {
            Err(CheckError::Mismatch(why)) => assert!(why.contains(&pin)),
            other => panic!("expected a mismatch naming the pin, got {other:?}"),
        }
        // Move the detached HEAD back to the pin, so the next check is only about an added tag.
        run_cmd("git", &["-C", &d, "update-ref", "HEAD", &pin]).unwrap();
        assert!(check_entry(&entry, &cache).is_ok());

        // adding a tag to the cache fails the check, naming the refs hash.
        git_cmd(&["update-ref", "refs/tags/sneaked-in", &pin], &dest);
        match check_entry(&entry, &cache) {
            Err(CheckError::Mismatch(why)) => {
                assert!(why.contains("refs hash") && why.contains(&refs_hash))
            }
            other => panic!("expected a refs-hash mismatch, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_failed_fetch_leaves_no_partial_and_no_entry() {
        let root = scratch("git-fail");
        let entry = Entry {
            kind: "git".to_string(),
            name: "test-git-fail".to_string(),
            url: root.join("does-not-exist").display().to_string(),
            license: "MIT".to_string(),
            pin: "0000000000000000000000000000000000000000".to_string(),
            hash: None,
            refs: None,
            note: String::new(),
        };
        let cache = root.join("cache");
        assert!(fetch_entry(&entry, &cache).is_err());
        let dest = cache.join("git").join("test-git-fail");
        assert!(
            !dest.exists(),
            "the entry must not exist after a failed fetch"
        );
        assert!(
            !partial_path(&dest).exists(),
            "no .partial must remain after a failed fetch"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Skipped where `rsync` is absent (not installed on the `windows-latest` CI runner), as the other
    /// real-tool tests in this workspace do.
    #[test]
    fn cvs_entry_fetch_check_corruption_and_no_network() {
        if Command::new("rsync").arg("--version").output().is_err() {
            eprintln!("skipping: rsync not on PATH");
            return;
        }
        let root = scratch("cvs");
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.txt"), "a\n").unwrap();
        std::fs::write(source.join("b.txt"), "b\n").unwrap();
        let hash = tree_sha256(&source).unwrap();

        let entry = Entry {
            kind: "cvs".to_string(),
            name: "test-cvs".to_string(),
            url: format!("{}/", source.display()),
            license: "MIT".to_string(),
            pin: "2026-01-01".to_string(),
            hash: Some(hash.clone()),
            refs: None,
            note: String::new(),
        };
        let cache = root.join("cache");

        assert!(matches!(
            fetch_entry(&entry, &cache),
            Ok(FetchOutcome::Fetched)
        ));
        assert!(check_entry(&entry, &cache).is_ok());

        // a second fetch changes nothing: it must not re-run rsync, only report AlreadyPresent.
        assert!(matches!(
            fetch_entry(&entry, &cache),
            Ok(FetchOutcome::AlreadyPresent)
        ));

        // the check passes with the source deleted: it needs no network.
        std::fs::remove_dir_all(&source).unwrap();
        assert!(check_entry(&entry, &cache).is_ok());

        // a corrupted cache fails the check, naming the entry.
        let dest = cache.join("cvs").join("test-cvs");
        std::fs::write(dest.join("a.txt"), "tampered\n").unwrap();
        match check_entry(&entry, &cache) {
            Err(CheckError::Mismatch(why)) => assert!(why.contains(&hash)),
            other => panic!("expected a hash mismatch, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cvs_failed_fetch_leaves_no_partial_and_no_entry() {
        let root = scratch("cvs-fail");
        let entry = Entry {
            kind: "cvs".to_string(),
            name: "test-cvs-fail".to_string(),
            url: format!("{}/", root.join("does-not-exist").display()),
            license: "MIT".to_string(),
            pin: "2026-01-01".to_string(),
            hash: Some("0".repeat(64)),
            refs: None,
            note: String::new(),
        };
        let cache = root.join("cache");
        assert!(fetch_entry(&entry, &cache).is_err());
        let dest = cache.join("cvs").join("test-cvs-fail");
        assert!(
            !dest.exists(),
            "the entry must not exist after a failed fetch"
        );
        assert!(
            !partial_path(&dest).exists(),
            "no .partial must remain after a failed fetch"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `svnrdump` needs a real server for most URL schemes, but accepts `file://` with no network
    /// involved — the same local-disk operation `svnadmin dump` would do. Skipped where `svnadmin` or
    /// `svnrdump` is absent, as the other real-tool tests in this workspace do.
    #[test]
    fn svn_entry_fetch_check_corruption_and_no_network() {
        if Command::new("svnadmin").arg("--version").output().is_err()
            || Command::new("svnrdump").arg("--version").output().is_err()
        {
            eprintln!("skipping: svnadmin/svnrdump not on PATH");
            return;
        }
        let root = scratch("svn");
        let repo = root.join("repo");
        let status = Command::new("svnadmin")
            .args(["create", &repo.display().to_string()])
            .status()
            .expect("spawn svnadmin");
        assert!(status.success());
        let url = format!("file://{}", repo.display());
        let status = Command::new("svn")
            .args(["mkdir", &format!("{url}/trunk"), "-m", "init"])
            .status()
            .expect("spawn svn");
        assert!(status.success());

        // Compute the expected hash once, the same way fetch_entry will, from a throwaway dump.
        let probe = root.join("probe.dump");
        {
            let file = std::fs::File::create(&probe).unwrap();
            let status = Command::new("svnrdump")
                .args(["dump", &url, "-r", "0:1"])
                .stdout(file)
                .status()
                .expect("spawn svnrdump");
            assert!(status.success());
        }
        let hash = canonical_svn_dump_hash(&std::fs::read(&probe).unwrap()).unwrap();
        std::fs::remove_file(&probe).unwrap();

        let entry = Entry {
            kind: "svn".to_string(),
            name: "test-svn".to_string(),
            url: url.clone(),
            license: "MIT".to_string(),
            pin: "1".to_string(),
            hash: Some(hash.clone()),
            refs: None,
            note: String::new(),
        };
        let cache = root.join("cache");

        assert!(matches!(
            fetch_entry(&entry, &cache),
            Ok(FetchOutcome::Fetched)
        ));
        assert!(check_entry(&entry, &cache).is_ok());

        // a second fetch changes nothing: it must not re-run svnrdump, only report AlreadyPresent.
        assert!(matches!(
            fetch_entry(&entry, &cache),
            Ok(FetchOutcome::AlreadyPresent)
        ));

        // the check passes with the source repository deleted: it needs no network.
        std::fs::remove_dir_all(&repo).unwrap();
        assert!(check_entry(&entry, &cache).is_ok());

        // a corrupted cache (a dump file placed by the test, still well-formed) fails the check,
        // naming the entry's recorded hash.
        let dest = cache.join("svn").join("test-svn").with_extension("dump");
        std::fs::write(&dest, b"SVN-fs-dump-format-version: 2\n\nUUID: 0\n\n").unwrap();
        match check_entry(&entry, &cache) {
            Err(CheckError::Mismatch(why)) => assert!(why.contains(&hash)),
            other => panic!("expected a dump-hash mismatch, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn svn_failed_fetch_leaves_no_partial_and_no_entry() {
        if Command::new("svnrdump").arg("--version").output().is_err() {
            eprintln!("skipping: svnrdump not on PATH");
            return;
        }
        let root = scratch("svn-fail");
        let entry = Entry {
            kind: "svn".to_string(),
            name: "test-svn-fail".to_string(),
            url: format!("file://{}", root.join("does-not-exist").display()),
            license: "MIT".to_string(),
            pin: "1".to_string(),
            hash: Some("0".repeat(64)),
            refs: None,
            note: String::new(),
        };
        let cache = root.join("cache");
        assert!(fetch_entry(&entry, &cache).is_err());
        let dest = cache
            .join("svn")
            .join("test-svn-fail")
            .with_extension("dump");
        assert!(
            !dest.exists(),
            "the entry must not exist after a failed fetch"
        );
        assert!(
            !partial_path(&dest).exists(),
            "no .partial must remain after a failed fetch"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_entry_reports_absent_for_an_empty_cache() {
        let root = scratch("absent");
        let entry = Entry {
            kind: "git".to_string(),
            name: "nothing-here".to_string(),
            url: "unused".to_string(),
            license: "MIT".to_string(),
            pin: "unused".to_string(),
            hash: None,
            refs: None,
            note: String::new(),
        };
        assert!(matches!(
            check_entry(&entry, &root.join("cache")),
            Err(CheckError::Absent)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- review 128 S-3: the canonical SVN-dump hash, hand-built dumps (no real svnrdump/svnadmin
    // needed — this is purely about the parser and the stable sort) ------------------------------------

    fn svn_preamble() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"SVN-fs-dump-format-version: 2\n\n");
        b.extend(b"UUID: 00000000-0000-0000-0000-000000000000\n\n");
        b
    }

    fn svn_revision(num: u64) -> Vec<u8> {
        let props = b"PROPS-END\n";
        let mut b = Vec::new();
        b.extend(format!("Revision-number: {num}\n").into_bytes());
        b.extend(
            format!(
                "Prop-content-length: {}\nContent-length: {}\n\n",
                props.len(),
                props.len()
            )
            .into_bytes(),
        );
        b.extend(props);
        b.extend(b"\n");
        b
    }

    fn svn_node_add(path: &str, content: &[u8]) -> Vec<u8> {
        let props = b"PROPS-END\n";
        let mut b = Vec::new();
        b.extend(format!("Node-path: {path}\nNode-kind: file\nNode-action: add\n").into_bytes());
        b.extend(
            format!(
                "Prop-content-length: {}\nText-content-length: {}\nContent-length: {}\n\n",
                props.len(),
                content.len(),
                props.len() + content.len()
            )
            .into_bytes(),
        );
        b.extend(props);
        b.extend(content);
        b.extend(b"\n\n");
        b
    }

    fn svn_node_delete(path: &str) -> Vec<u8> {
        format!("Node-path: {path}\nNode-action: delete\n\n").into_bytes()
    }

    #[test]
    fn canonical_svn_hash_ignores_node_order_within_a_revision() {
        let mut a = svn_preamble();
        a.extend(svn_revision(1));
        a.extend(svn_node_add("a.txt", b"A"));
        a.extend(svn_node_add("b.txt", b"B"));

        let mut b = svn_preamble();
        b.extend(svn_revision(1));
        b.extend(svn_node_add("b.txt", b"B"));
        b.extend(svn_node_add("a.txt", b"A"));

        assert_ne!(a, b, "the two dumps must actually differ in node order");
        assert_eq!(
            canonical_svn_dump_hash(&a).unwrap(),
            canonical_svn_dump_hash(&b).unwrap()
        );
    }

    #[test]
    fn canonical_svn_hash_changes_with_a_changed_content_byte() {
        let mut a = svn_preamble();
        a.extend(svn_revision(1));
        a.extend(svn_node_add("a.txt", b"A"));

        let mut b = svn_preamble();
        b.extend(svn_revision(1));
        b.extend(svn_node_add("a.txt", b"Z"));

        assert_ne!(
            canonical_svn_dump_hash(&a).unwrap(),
            canonical_svn_dump_hash(&b).unwrap()
        );
    }

    #[test]
    fn canonical_svn_hash_changes_with_reordered_revisions() {
        let mut a = svn_preamble();
        a.extend(svn_revision(1));
        a.extend(svn_node_add("a.txt", b"A"));
        a.extend(svn_revision(2));
        a.extend(svn_node_add("b.txt", b"B"));

        // The revisions themselves swapped — not realistic for a real dump, but this is exactly what
        // the canonicalization must *not* ignore, unlike node order within one revision.
        let mut b = svn_preamble();
        b.extend(svn_revision(2));
        b.extend(svn_node_add("b.txt", b"B"));
        b.extend(svn_revision(1));
        b.extend(svn_node_add("a.txt", b"A"));

        assert_ne!(
            canonical_svn_dump_hash(&a).unwrap(),
            canonical_svn_dump_hash(&b).unwrap()
        );
    }

    #[test]
    fn canonical_svn_hash_keeps_the_order_of_records_on_one_path() {
        // A "replace" (delete, then add, on the same path): the stable sort must leave two records
        // that share a sort key (the same `Node-path`) in their original relative order, so this is
        // never silently reordered into "add, then delete".
        let mut delete_then_add = svn_preamble();
        delete_then_add.extend(svn_revision(1));
        delete_then_add.extend(svn_node_delete("a.txt"));
        delete_then_add.extend(svn_node_add("a.txt", b"NEW"));

        let mut add_then_delete = svn_preamble();
        add_then_delete.extend(svn_revision(1));
        add_then_delete.extend(svn_node_add("a.txt", b"NEW"));
        add_then_delete.extend(svn_node_delete("a.txt"));

        assert_ne!(
            canonical_svn_dump_hash(&delete_then_add).unwrap(),
            canonical_svn_dump_hash(&add_then_delete).unwrap(),
            "a stable sort must not reorder same-path records relative to each other"
        );
    }
}
