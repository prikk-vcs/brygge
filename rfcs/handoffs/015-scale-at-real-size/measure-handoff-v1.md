# Handoff — RFC 015 D-1: attribute a real decode's peak memory (batch A, measure first, no change)

**Governing:** RFC 015 (accepted 2026-10-20). **Tools only** (`tools/bench`, `publish = false`); **no change to any
published crate.** One review request. Batches W, S and V are written from this batch's numbers, as RFC 010's
increments were.

## 0. First, commit the architect's files on their own

Commit as "RFC 015 accepted (Scale at real size, 0.5.0); 0.6.0 planned; handoff":
- `rfcs/done/015-scale-at-real-size.md` (moved from `proposed/`);
- `rfcs/handoffs/015-scale-at-real-size/`;
- `rfcs/README.md`;
- `ROADMAP.md`.

The links resolve on that commit alone.

## 1. What to measure

**Where the peak goes, by phase**, for:
- **fish-shell** and **TortoiseHg** (the corpus's two largest), and
- the largest synthetic scenario per kind (`git-commits`, `cvs-revs`, `svn-dump`, the hg equivalent).

| Phase | How (each in its own subprocess, the bench's method) |
|---|---|
| **P1, decode only** | call the decoder through the library, keep the `Ir`, stop (no serialization) |
| **P2, decode and serialize** | P1, then `artifact::to_bytes`, then stop (no file write) |
| **P3, the full CLI decode** | `brygge decode … --out` (what users run) |
| **P4, `verify`** | `brygge verify <artifact>` |
| **P5, `inspect`** | `brygge inspect <artifact>` |
| **P6, `verify --against-source`** | the full re-decode and comparison |

For each phase, record the peak (`VmHWM`), the time, and the resident-memory **timeline**: sample `/proc/self/statm`
in a background thread every 50 ms, and mark the phase boundaries. A peak alone cannot show *which* holder was
alive at that moment.

## 2. The attribution table (the deliverable)

Per row, record:
- the content bytes, the metadata bytes (`encode_metadata().len()`), the artifact bytes, and the atoms;
- P1…P6 peaks;
- and the **derived holders:**
  - **the content store** (about the content bytes);
  - **the decoder's working set** (P1 − content − metadata);
  - **the serialization buffers** (P2 − P1);
  - **the file write** (P3 − P2);
  - **the reader's copy** (P4 and P5).
- **State plainly what does not add up,** if anything. An unexplained gigabyte is a finding, not noise.

## 3. Also report (they decide D-3's open details)

- **The cost of a spill on the bench:** write every blob of fish-shell's IR to a file, then read it back in id
  order. Measure the time and the peak against today's in-memory path. That tells the architect whether small
  decodes should spill too (RFC 015 D-3).
- **The per-atom metadata cost** on the real corpus (the metadata bytes ÷ atoms). RFC 010's 5 KiB-per-atom figure
  was synthetic.
- **The machine,** and the free disk on the output filesystem during the runs.

## 4. Proof and placement

- The tooling: a `tools/bench` subcommand (for example `profile`), using only the public library APIs and
  `brygge-bench`'s existing harness.
- The results go in `tools/bench/README.md`, in a section "RFC 015 D-1: where the peak goes".
- The usual gates, for the tools code. No published crate changes, so `cargo publish --workspace --dry-run`
  is unchanged.
- **No CHANGELOG entry:** it is a measurement, not a change users see.

## 5. Review request

`.git-exclude/review-requests/049-scale-attribution.md`. Then the architect writes batches W, S and V, with
their targets set from your table.
