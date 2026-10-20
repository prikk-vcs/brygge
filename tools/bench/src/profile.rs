//! RFC 015 D-1: where a real decode's peak memory goes, phase by phase. **Measurement only — no change
//! to any published crate** (the handoff's own scope). Every phase runs in its own process, polled from
//! outside (the same reason `real.rs`'s decodes do): a process that does its own measuring and its own
//! work shares one address space, so a later step's allocator retention can inflate an earlier step's
//! read-after-the-fact peak — this file's whole design avoids that by never measuring itself.
//!
//! - **P1, P2** (`profile-one p1|p2 <kind> <path>`): this binary, re-invoked as a child, does only the
//!   library-level work (decode; decode + serialize) and exits. The parent polls the child's own
//!   `/proc/<pid>/status` (`VmHWM`) and `/proc/<pid>/statm` (the timeline) from outside, so nothing the
//!   child prints afterward (its stats line) can contaminate the measured peak — the child's own
//!   allocations for printing happen only after its timed work is already done, and the parent's last
//!   poll before exit has already captured the true peak.
//! - **P3..P6**: the real `brygge` binary (`decode`, `verify`, `inspect`, `verify --against-source`),
//!   spawned and polled the same way — what a user actually runs.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use brygge_decode_cvs::{Options as CvsOpts, Source as CvsSource};
use brygge_decode_git::Options as GitOpts;
use brygge_decode_hg::Options as HgOpts;
use brygge_decode_svn::{Options as SvnOpts, Source as SvnSource};
use brygge_ir::Ir;

use crate::real::brygge_bin;

/// A decode source for this module's own purposes (not `crate::Source`, which has no `Hg` variant and
/// is wired to the synthetic-corpus generators, not the real-corpus cache paths this module reads).
#[derive(Clone)]
pub enum ProfileSource {
    Git(PathBuf),
    Hg(PathBuf),
    Cvs(PathBuf),
    Svn(PathBuf),
}

impl ProfileSource {
    fn kind(&self) -> &'static str {
        match self {
            Self::Git(_) => "git",
            Self::Hg(_) => "hg",
            Self::Cvs(_) => "cvs",
            Self::Svn(_) => "svn",
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Git(p) | Self::Hg(p) | Self::Cvs(p) | Self::Svn(p) => p,
        }
    }

    fn parse(kind: &str, path: &Path) -> Self {
        match kind {
            "git" => Self::Git(path.to_path_buf()),
            "hg" => Self::Hg(path.to_path_buf()),
            "cvs" => Self::Cvs(path.to_path_buf()),
            "svn" => Self::Svn(path.to_path_buf()),
            other => panic!("unknown profile source kind {other}"),
        }
    }
}

fn decode_source(src: &ProfileSource) -> Ir {
    match src {
        ProfileSource::Git(p) => {
            brygge_decode_git::decode(p, &GitOpts::default()).expect("git decode")
        }
        ProfileSource::Hg(p) => brygge_decode_hg::decode(p, &HgOpts::default()).expect("hg decode"),
        ProfileSource::Cvs(p) => {
            brygge_decode_cvs::decode(&CvsSource::LocalRepo(p.clone()), &CvsOpts::default())
                .expect("cvs decode")
        }
        ProfileSource::Svn(p) => {
            brygge_decode_svn::decode(&SvnSource::DumpFile(p.clone()), &SvnOpts::default())
                .expect("svn decode")
        }
    }
}

/// One named target: a label for the report, and where to decode it from.
pub struct Target {
    pub label: &'static str,
    pub source: ProfileSource,
}

// ---- the six phases, orchestrated ------------------------------------------------------------------

const PHASES: &[&str] = &["P1", "P2", "P3", "P4", "P5", "P6"];

/// Run every phase for every target, printing one machine line per (target, phase), plus the target's
/// own content/metadata/artifact/atom counts (from P2, the only phase that serializes).
pub fn run(targets: &[Target], out_dir: &Path) {
    std::fs::create_dir_all(out_dir).expect("create the output directory");
    let exe = std::env::current_exe().expect("current exe");
    let bin = brygge_bin();

    for target in targets {
        let kind = target.source.kind();
        let path = target.source.path();
        let artifact = out_dir.join(format!("{}.artifact", target.label));

        for &phase in PHASES {
            let mut cmd = match phase {
                "P1" => {
                    let mut c = Command::new(&exe);
                    c.args(["profile-one", "p1", kind, &path.display().to_string()]);
                    c
                }
                "P2" => {
                    let mut c = Command::new(&exe);
                    c.args(["profile-one", "p2", kind, &path.display().to_string()]);
                    c
                }
                "P3" => {
                    let mut c = Command::new(&bin);
                    c.args([
                        "decode",
                        kind,
                        &path.display().to_string(),
                        "--out",
                        &artifact.display().to_string(),
                    ]);
                    c
                }
                "P4" => {
                    let mut c = Command::new(&bin);
                    c.args(["verify", &artifact.display().to_string()]);
                    c
                }
                "P5" => {
                    let mut c = Command::new(&bin);
                    c.args(["inspect", &artifact.display().to_string()]);
                    c
                }
                "P6" => {
                    let mut c = Command::new(&bin);
                    c.args([
                        "verify",
                        &artifact.display().to_string(),
                        "--against-source",
                        &path.display().to_string(),
                    ]);
                    c
                }
                other => unreachable!("{other}"),
            };
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            let measured = measure(cmd);
            println!(
                "target={} phase={phase} ok={} time_ms={} peak_kib={} samples={}{}",
                target.label,
                measured.ok,
                measured.time_ms,
                measured.peak_kib,
                measured.timeline.len(),
                measured.stdout_tail,
            );
            if !measured.ok {
                eprintln!(
                    "target={} phase={phase}: child failed: {}",
                    target.label, measured.stderr
                );
            }
        }
    }
}

struct Measured {
    ok: bool,
    time_ms: u128,
    peak_kib: u64,
    timeline: Vec<(u64, u64)>,
    /// P1/P2's own stats line (content/metadata/artifact/atoms), appended verbatim if present.
    stdout_tail: String,
    stderr: String,
}

/// Spawn `cmd`, poll its `/proc/<pid>/status` every 50 ms until it exits, and record the peak `VmHWM`
/// and the timeline. The measurement is **external**: this process never does the measured work itself.
fn measure(mut cmd: Command) -> Measured {
    let start = Instant::now();
    let mut child: Child = cmd.spawn().expect("spawn");
    let pid = child.id();
    let mut peak_kib = 0u64;
    let mut timeline = Vec::new();
    loop {
        if let Some(kib) = read_vmhwm(pid) {
            peak_kib = peak_kib.max(kib);
            timeline.push((
                u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                kib,
            ));
        }
        if let Some(status) = child.try_wait().expect("try_wait") {
            let time_ms = start.elapsed().as_millis();
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_string(&mut stdout);
            }
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_string(&mut stderr);
            }
            let stdout_tail = stdout
                .lines()
                .find(|l| l.starts_with("content_bytes="))
                .map(|l| format!(" {l}"))
                .unwrap_or_default();
            // `brygge`'s own exit codes (`crates/brygge/src/exit.rs`): 0 (clean) and 10 (recorded
            // loss — every real repository has some; batch R's own runs show `dropped>0` on every
            // entry) both mean the command did its job and produced a result. Anything else (a
            // refusal, a verify failure, a genuine crash) is a real problem for this measurement.
            let ok = matches!(status.code(), Some(0) | Some(10));
            return Measured {
                ok,
                time_ms,
                peak_kib,
                timeline,
                stdout_tail,
                stderr,
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_vmhwm(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

// ---- profile-one: the child's own side ------------------------------------------------------------

/// `profile-one p1|p2 <kind> <path>`: the measured work alone, then exit. The parent process (`run`,
/// above) does all the measuring; this function must do nothing after the timed step except read
/// already-computed, already-allocated values (`ir.atoms.len()`, `ir.content.total_bytes()`), never a
/// fresh allocation of consequence, so printing never contaminates the peak the parent already captured.
pub fn run_one(phase: &str, kind: &str, path: &Path) {
    let src = ProfileSource::parse(kind, path);
    match phase {
        "p1" => {
            let ir = decode_source(&src);
            println!(
                "content_bytes={} atoms={}",
                ir.content.total_bytes(),
                ir.atoms.len()
            );
        }
        "p2" => {
            let ir = decode_source(&src);
            let artifact = brygge_ir::artifact::to_bytes(&ir);
            let metadata_bytes = metadata_len(&artifact);
            println!(
                "content_bytes={} atoms={} metadata_bytes={} artifact_bytes={}",
                ir.content.total_bytes(),
                ir.atoms.len(),
                metadata_bytes,
                artifact.len()
            );
        }
        other => panic!("unknown profile-one phase {other}"),
    }
}

/// The exact length of the metadata record inside a serialized artifact, read forward from its own
/// documented layout (`crates/brygge-ir/src/artifact.rs`'s module doc: `magic(8) · format(1) ·
/// version(uvarint×3) · digest(32)`, then `[len] metadata`) — `Ir::encode_metadata` is `pub(crate)` in
/// `brygge-ir`, so this reads the length brygge itself wrote, rather than needing crate access to ask
/// for it directly (this module uses only `brygge-ir`'s public API, per the handoff).
fn metadata_len(artifact: &[u8]) -> u64 {
    let mut pos = 8 + 1; // magic, format
    for _ in 0..3 {
        // major, minor, patch
        let (_, n) = read_uvarint(&artifact[pos..]);
        pos += n;
    }
    pos += 32; // digest
    let (len, _) = read_uvarint(&artifact[pos..]);
    len
}

fn read_uvarint(bytes: &[u8]) -> (u64, usize) {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut i = 0usize;
    loop {
        let byte = bytes[i];
        i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (value, i)
}

// ---- the spill-cost one-off (handoff §3) -----------------------------------------------------------

/// Write every blob of `source`'s IR to its own file under `spill_dir` (id order — already
/// `iter_sorted`'s order), **drop the decoded `Ir` entirely**, then read every blob back from disk one
/// at a time (never accumulating more than one in memory), simulating how a disk-backed store would
/// actually be read on demand. Prints the in-memory decode's own peak/time, the write peak/time (while
/// the whole `Ir` is still resident — writing alone, not yet the benefit), and the **post-drop**
/// read-back peak/time, which is where a real reduction would show (RFC 015 D-3).
pub fn run_spill(kind: &str, path: &Path, spill_dir: &Path) {
    use crate::{peak_rss_kib, reset_peak_rss};

    std::fs::create_dir_all(spill_dir).expect("create the spill directory");
    let src = ProfileSource::parse(kind, path);

    reset_peak_rss();
    let t0 = Instant::now();
    let ir = decode_source(&src);
    let decode_ms = t0.elapsed().as_millis();
    let decode_peak = peak_rss_kib();
    let blobs = ir.content.len();
    let content_bytes = ir.content.total_bytes();

    reset_peak_rss();
    let t1 = Instant::now();
    let mut ids: Vec<String> = Vec::with_capacity(blobs);
    for (id, bytes) in ir.content.iter_sorted() {
        let file = spill_dir.join(id.to_hex());
        std::fs::write(&file, bytes).expect("write a spilled blob");
        ids.push(id.to_hex());
    }
    let write_ms = t1.elapsed().as_millis();
    let write_peak = peak_rss_kib();

    // The whole point: drop the decoded `Ir` (and so its content store) before reading anything back,
    // so the next phase starts from a cold, `Ir`-free baseline — what spilling is actually for.
    drop(ir);
    reset_peak_rss();
    let t2 = Instant::now();
    let mut total_read = 0u64;
    for hex in &ids {
        let bytes = std::fs::read(spill_dir.join(hex)).expect("read a spilled blob back");
        total_read += bytes.len() as u64;
        drop(bytes); // one blob in memory at a time, as a disk-backed store would read it on demand
    }
    let read_ms = t2.elapsed().as_millis();
    let read_peak = peak_rss_kib();

    println!(
        "decode_time_ms={decode_ms} decode_peak_kib={} write_time_ms={write_ms} write_peak_kib={} \
         read_time_ms={read_ms} read_peak_kib={} blobs={blobs} content_bytes={content_bytes} read_back_bytes={total_read}",
        decode_peak.map_or_else(|| "n/a".to_string(), |k| k.to_string()),
        write_peak.map_or_else(|| "n/a".to_string(), |k| k.to_string()),
        read_peak.map_or_else(|| "n/a".to_string(), |k| k.to_string()),
    );
}
