//! Compute each changeset's phase from `.hg/store/phaseroots` (RFC 005 corrections handoff §1.1):
//! public (`0`) by default, or the **maximum** phase of the roots a changeset descends from (or is).
//! An absent file means everything is public; a node named in the file but not in the changelog is
//! ignored (it may name a changeset stripped since the roots file was written).

use std::collections::HashMap;
use std::path::Path;

use crate::Error;
use crate::published::{Limits, read_bounded};
use crate::revlog::{NULL_REV, Revlog};
use crate::util::parse_hex20;

/// Every changeset with phase `>=` this value is not part of the published view (secret, archived,
/// internal). Phase `1` (draft) is a normal, shareable phase and stays published.
pub(crate) const NOT_PUBLISHED_FLOOR: u32 = 2;

/// The only phase values Mercurial defines (review 009 R-6): public is the implicit `0` default, never
/// written to `phaseroots` itself.
const KNOWN_PHASES: [u32; 4] = [1, 2, 32, 96];

/// Compute the phase of every changeset in `changelog`, indexed by revision number.
///
/// # Errors
/// [`Error::Open`] if `phaseroots` exists but cannot be stat'd/read; [`Error::Read`] on a malformed
/// line, an unknown phase value, or non-UTF-8 content — a store that cannot say what is secret must not
/// be decoded as if nothing were; [`Error::ResourceLimit`] if the file exceeds
/// `limits.max_small_file_bytes`.
pub(crate) fn compute(
    store: &Path,
    changelog: &Revlog,
    limits: &Limits,
) -> Result<Vec<u32>, Error> {
    let n = changelog.len();
    let mut phase = vec![0u32; n];

    let path = store.join("phaseroots");
    let Some(data) = read_bounded(&path, "the phaseroots file", limits)? else {
        return Ok(phase);
    };
    let roots = parse_phaseroots(&data)?;

    let mut node_to_rev: HashMap<[u8; 20], usize> = HashMap::with_capacity(n);
    for rev in 0..n {
        if let Some(entry) = changelog.entry(rev) {
            node_to_rev.insert(entry.node, rev);
        }
    }

    for (declared, node) in roots {
        // "Any node not present in the changelog is ignored."
        if let Some(&rev) = node_to_rev.get(&node) {
            if let Some(slot) = phase.get_mut(rev) {
                *slot = (*slot).max(declared);
            }
        }
    }

    // Forward pass in ascending rev order: parents always have a lower rev number than their children
    // in a revlog, so by the time `rev` is processed, `phase[p1]`/`phase[p2]` already reflect every
    // ancestor's own root (direct or inherited) — one O(n) pass computes "max phase of roots descended
    // from" exactly.
    for rev in 0..n {
        let Some(entry) = changelog.entry(rev) else {
            continue;
        };
        let mut inherited = 0u32;
        for p in [entry.p1, entry.p2] {
            if p != NULL_REV {
                if let Ok(pi) = usize::try_from(p) {
                    inherited = inherited.max(phase.get(pi).copied().unwrap_or(0));
                }
            }
        }
        if let Some(slot) = phase.get_mut(rev) {
            *slot = (*slot).max(inherited);
        }
    }

    Ok(phase)
}

/// Parse `phaseroots`' text into `(declared phase, node)` pairs, with no filesystem access and no
/// changelog needed — the one place [`compute`] does the real parse, after reading the file. Also used
/// directly by mutation tests (RFC 014 D-1) to avoid a temp-file round trip per iteration.
///
/// # Errors
/// [`Error::Read`] on non-UTF-8 content, a malformed line, or an unknown phase value.
pub(crate) fn parse_phaseroots(data: &[u8]) -> Result<Vec<(u32, [u8; 20])>, Error> {
    let body = std::str::from_utf8(data)
        .map_err(|_| Error::Read("the phaseroots file is not valid UTF-8".to_string()))?;
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (phase_str, node_hex) = line
            .split_once(' ')
            .ok_or_else(|| Error::Read(format!("malformed phaseroots line {line:?}")))?;
        let declared: u32 = phase_str
            .parse()
            .map_err(|_| Error::Read(format!("malformed phaseroots line {line:?}")))?;
        if !KNOWN_PHASES.contains(&declared) {
            return Err(Error::Read(format!(
                "unknown phaseroots phase value {declared} in {line:?}"
            )));
        }
        out.push((declared, parse_hex20(node_hex)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mutation;
