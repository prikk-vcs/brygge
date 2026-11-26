//! Checking `encode git`'s result against the artifact it came from (RFC 019 D-6, batch G2): the same
//! comparison serves `encode git`'s own in-process check (run before the result is kept) and the
//! standalone `verify --against-target git <repository>` (over a repository someone kept). **Isolated to
//! `brygge_ir` and `std` alone**, like the rest of `encode_git` (the module doc on `mod.rs`): finding
//! `git`, decoding the result repository, and listing its raw refs are the CLI's own job
//! (`commands.rs`), which hands this module two already-decoded `Ir`s plus the result's raw ref listing.
//!
//! **Matching, not re-deriving:** this never re-runs `encode_git::write_stream`. It matches each result
//! commit to an artifact atom by the `Brygge-Source-Id` trailer F-7 defined, then compares **states**,
//! never ops — two decoders can represent one resulting tree with different op sequences (an SVN replace
//! against a Git modify; a no-op modify Git never records), so only the path → (mode, content id) state
//! after each commit is ever compared, and content ids are the IR's own SHA-256 addresses (`BlobId`), so
//! equal bytes already give equal ids on both sides with no blob read at all.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use brygge_ir::{
    AtomId, BlobId, ChangeAtom, Content, Identity, Ir, PathOp, RefKind, RefRecord, SourceKind,
    Text, Time,
};

use super::trailers;

/// One ref as `commands.rs` lists it from the real repository (`git for-each-ref`, plus `HEAD`), under
/// G1's own fixed environment — read-only, no user configuration. The Git decoder carries only
/// `refs/heads/*` and `refs/tags/*` (as `result.refs`); everything else, `refs/brygge/other/*` and
/// `HEAD` included, is read only from this listing, which is also how "nothing extra" (no ref beyond the
/// artifact's own mapping) is checked: every ref the repository actually has must be named here, in every
/// namespace, including the ones `result.refs` already covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitRef {
    /// The ref's full name (`refs/heads/main`, `refs/brygge/other/x`, or the literal `"HEAD"`).
    pub(crate) name: String,
    /// What it points at.
    pub(crate) target: GitRefTarget,
}

/// What a [`GitRef`] resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitRefTarget {
    /// A 20-byte object id (annotated tags point at the tag object, not the commit it tags).
    Oid(Vec<u8>),
    /// `HEAD`, when it is a symbolic ref (the ordinary case): the full name of the ref it names.
    Symbolic(String),
}

/// Names a difference the way a person investigating it can actually look it up (review 142 N-1): the
/// artifact's own `Brygge-Source-Id`, in F-7's readable form and truncated exactly as a refusal message
/// is (`trailers::truncate_for_refusal`) — never the IR's internal `AtomId`, which nobody outside brygge
/// itself can look up at all.
fn diff_source_label(source_kind: &SourceKind, source_atom_id: &[u8]) -> String {
    format!(
        "source {}",
        trailers::truncate_for_refusal(&trailers::source_id_text(source_kind, source_atom_id))
    )
}

/// The result's own commit id (always Git, in this module), named the same truncated way, as a suffix
/// onto [`diff_source_label`] when a result commit is actually known for the row (review 142 N-1: "add
/// the result's commit id where there is one").
fn diff_commit_suffix(commit_atom_id: &[u8]) -> String {
    format!(
        " (commit {})",
        trailers::truncate_for_refusal(&trailers::to_hex(commit_atom_id))
    )
}

/// The result of [`compare_git_target`].
#[derive(Debug)]
pub(crate) enum Comparison {
    /// Every row of RFC 019 D-6 §2 held.
    Corresponds,
    /// At least one difference, named — `encode_git`'s own check reports at most 20, then a count;
    /// `verify --against-target` reports the same list.
    DoesNotCorrespond(Vec<String>),
    /// The comparison itself could not run (not a claim of tampering — review 003 R-1's rule, extended
    /// to this comparison): two artifact atoms share one source id, so matching cannot be trusted.
    NotChecked(String),
}

/// Every atom's resulting tree state, incrementally: `path → (mode, content id)` after that atom's own
/// ops, built from its first parent's state (RFC 019 D-6 §4 — a merge's `M`/`D` ops are always relative
/// to the `from` parent, exactly as `fast-import` and `write_stream` both treat them). `Rc`-shared so a
/// branch point costs one pointer clone, not a map clone, until a child actually mutates it.
type TreeState = BTreeMap<String, (u32, BlobId)>;

fn apply_ops(state: &mut TreeState, ops: &[PathOp]) {
    for op in ops {
        match op {
            PathOp::Add { path, content, .. }
            | PathOp::Modify { path, content, .. }
            | PathOp::Replace { path, content, .. } => match content {
                // A submodule's state is not yet tracked here (RFC 021 D-6 defers this comparison to
                // batch G, alongside the Git decoder that would first produce one) — `encode_git`'s own
                // `preflight` already refuses any artifact holding one before this ever runs.
                Content::Blob { blob, mode } => {
                    state.insert(path.clone(), (*mode, *blob));
                }
                Content::Submodule { .. } => {}
            },
            PathOp::Delete { path, .. } => {
                state.remove(path);
            }
        }
    }
}

/// How many times each atom is named as a parent — a child uses its first parent's tree state, and a
/// comparison (if the atom is matched) uses it once more. An atom's own entry here is removed, dropping
/// its `Rc`, once nothing further needs it (RFC 019 D-6 §4's bound: no tree state is held for the whole
/// history).
fn parent_use_counts(atoms: &[ChangeAtom]) -> HashMap<AtomId, u32> {
    let mut counts = HashMap::new();
    for atom in atoms {
        if let Some(first) = atom.parents.first() {
            *counts.entry(*first).or_insert(0) += 1;
        }
    }
    counts
}

/// One side's incremental tree-state walk (RFC 019 D-6 §4): `ancestor` holds every atom still needed as
/// a parent by an atom not yet produced; `pending` holds, additionally, every **matched** atom's state
/// until its cross-side comparison has actually run. The two are tracked separately so a state already
/// freed from `ancestor` (no remaining child) can still be held in `pending` a little longer, and vice
/// versa.
struct Side<'a> {
    atoms: &'a [ChangeAtom],
    by_id: HashMap<AtomId, &'a ChangeAtom>,
    next: usize,
    ancestor: HashMap<AtomId, Rc<TreeState>>,
    ancestor_remaining: HashMap<AtomId, u32>,
    pending: HashMap<AtomId, Rc<TreeState>>,
}

impl<'a> Side<'a> {
    fn new(atoms: &'a [ChangeAtom]) -> Self {
        Self {
            atoms,
            by_id: atoms.iter().map(|a| (a.id, a)).collect(),
            next: 0,
            ancestor: HashMap::new(),
            ancestor_remaining: parent_use_counts(atoms),
            pending: HashMap::new(),
        }
    }

    fn done(&self) -> bool {
        self.next >= self.atoms.len()
    }

    /// Produces the next atom's tree state, or does nothing if [`Self::done`]. `matched` marks which
    /// atom ids need their state kept for a cross-side comparison (beyond [`Self::ancestor`]'s own,
    /// independent lifetime).
    fn produce(&mut self, matched: &HashMap<AtomId, AtomId>) {
        let Some(atom) = self.atoms.get(self.next) else {
            return;
        };
        self.next += 1;
        let mut state = match atom.parents.first() {
            Some(p) => self
                .ancestor
                .get(p)
                .map(|rc| (**rc).clone())
                .unwrap_or_default(),
            None => TreeState::new(),
        };
        apply_ops(&mut state, &atom.ops);
        let rc = Rc::new(state);
        self.ancestor.insert(atom.id, rc.clone());
        if matched.contains_key(&atom.id) {
            self.pending.insert(atom.id, rc);
        }
        // Only the *first* parent's state was ever read (cloned) above — a merge parent's tree state is
        // never consumed by anyone, only its presence in `parents` (checked elsewhere). Decrementing for
        // every parent here, while `parent_use_counts` only counted first-parent uses, let a still-needed
        // ancestor be evicted early whenever one child used an atom as a merge parent and another, not
        // yet produced, still needed it as a first parent — corrupting every descendant's tree silently
        // (the "an atom's own op matches, but its tree differs anyway" bug a real fish-shell merge found).
        if let Some(first) = atom.parents.first() {
            if let Some(c) = self.ancestor_remaining.get_mut(first) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    self.ancestor.remove(first);
                }
            }
        }
    }
}

/// Compares atoms (RFC 019 D-6 §2's "matching" and per-matched-pair rows), producing both sides' tree
/// states in lockstep (one atom per side per round) so a matched pair is compared, and its states freed,
/// as soon as both are ready — never holding a side's whole history of states at once.
fn compare_atoms(ir: &Ir, result: &Ir, digest_hex: &str, diffs: &mut Vec<String>) {
    let mut artifact_to_result: HashMap<AtomId, AtomId> = HashMap::new();
    let mut result_to_artifact: HashMap<AtomId, AtomId> = HashMap::new();

    // Build the artifact's own id-by-source-bytes index, refusing up front (not-checked, review 003
    // R-1's pattern extended here) if it is not unique — the caller returns `NotChecked` for this, so
    // this function is only ever called once that has already been ruled out. Still matched here by
    // direct re-derivation from `ir`, so the two can never drift.
    let mut by_source_id: HashMap<Vec<u8>, AtomId> = HashMap::new();
    for atom in &ir.atoms {
        by_source_id.insert(atom.source.atom_id.clone(), atom.id);
    }
    // Resolves a result `AtomId` back to its own real Git commit id, for naming a second result commit
    // that claims an already-claimed artifact atom (review 142 N-1) — never the internal `AtomId` itself.
    let result_by_id: HashMap<AtomId, &ChangeAtom> =
        result.atoms.iter().map(|a| (a.id, a)).collect();

    for commit in &result.atoms {
        let message = commit
            .metadata
            .message
            .as_ref()
            .map(|t| t.bytes.as_slice())
            .unwrap_or(&[]);
        let label = || {
            format!(
                "commit {}",
                trailers::truncate_for_refusal(&trailers::source_id_text(
                    &SourceKind::Git,
                    &commit.source.atom_id
                ))
            )
        };
        let Some(value) = trailers::extract_trailer(message, "Brygge-Source-Id") else {
            diffs.push(format!("{}: has no Brygge-Source-Id trailer", label()));
            continue;
        };
        let Some(bytes) = trailers::parse_source_id(&ir.provenance.source.kind, &value) else {
            diffs.push(format!(
                "{}: its Brygge-Source-Id ({value:?}) does not parse as this source's id",
                label()
            ));
            continue;
        };
        let Some(&artifact_id) = by_source_id.get(&bytes) else {
            diffs.push(format!(
                "{}: its Brygge-Source-Id names no atom in the artifact",
                label()
            ));
            continue;
        };
        if let Some(existing) = artifact_to_result.insert(artifact_id, commit.id) {
            #[allow(clippy::expect_used)]
            // `existing` was inserted from `result.atoms` just above, in an earlier iteration of this
            // same loop; it cannot have been removed since.
            let existing_atom = result_by_id.get(&existing).expect("result's own atom id");
            diffs.push(format!(
                "{}: two result commits both claim it, commit {} and commit {}",
                diff_source_label(&ir.provenance.source.kind, &bytes),
                trailers::truncate_for_refusal(&trailers::to_hex(&existing_atom.source.atom_id)),
                trailers::truncate_for_refusal(&trailers::to_hex(&commit.source.atom_id)),
            ));
            continue;
        }
        result_to_artifact.insert(commit.id, artifact_id);
    }
    for atom in &ir.atoms {
        if !artifact_to_result.contains_key(&atom.id) {
            diffs.push(format!(
                "{}: no commit in the result names it",
                diff_source_label(&ir.provenance.source.kind, &atom.source.atom_id)
            ));
        }
    }

    let mut a_side = Side::new(&ir.atoms);
    let mut r_side = Side::new(&result.atoms);
    while !a_side.done() || !r_side.done() {
        if !a_side.done() {
            a_side.produce(&artifact_to_result);
        }
        if !r_side.done() {
            r_side.produce(&result_to_artifact);
        }
        let ready: Vec<(AtomId, AtomId)> = a_side
            .pending
            .keys()
            .filter_map(|aid| {
                let rid = *artifact_to_result.get(aid)?;
                r_side.pending.contains_key(&rid).then_some((*aid, rid))
            })
            .collect();
        for (aid, rid) in ready {
            // Both removals are guaranteed to find their entry: `ready` was built from exactly these
            // two maps, immediately above, with no mutation of either in between.
            #[allow(clippy::expect_used)]
            let a_state = a_side
                .pending
                .remove(&aid)
                .expect("just filtered as present");
            #[allow(clippy::expect_used)]
            let r_state = r_side
                .pending
                .remove(&rid)
                .expect("just filtered as present");
            #[allow(clippy::expect_used)]
            let artifact_atom = *a_side.by_id.get(&aid).expect("atom from its own IR");
            #[allow(clippy::expect_used)]
            let result_atom = *r_side.by_id.get(&rid).expect("atom from its own IR");
            compare_matched_pair(
                artifact_atom,
                result_atom,
                &a_state,
                &r_state,
                &artifact_to_result,
                &ir.transforms,
                &ir.provenance.source.kind,
                digest_hex,
                diffs,
            );
        }
    }
}

/// `trailers::identity_line`'s own output, decomposed back into `(name, email)` — reused rather than
/// re-implemented, so the expected value can never drift from what the writer actually emits (RFC 019
/// D-6 §2: a `Brygge-Supplied`/`Brygge-Altered` field must equal exactly what G1's rules write, not the
/// artifact's own, possibly-absent claim).
fn expected_identity(identity: Option<&Identity>, field: &'static str) -> (String, String) {
    let mut supplied = trailers::Supplied::default();
    let mut altered = trailers::Altered::default();
    let line = trailers::identity_line(identity, &mut supplied, &mut altered, field);
    let (name, email_part) = line.rsplit_once(" <").unwrap_or((line.as_str(), ">"));
    let email = email_part.strip_suffix('>').unwrap_or(email_part);
    (name.to_string(), email.to_string())
}

/// `trailers::when`'s own output, decomposed back into `(seconds, offset_minutes)` — the same reuse as
/// [`expected_identity`], for the same reason.
fn expected_when(
    time: Option<Time>,
    time_field: &'static str,
    tz_field: &'static str,
) -> (i64, i16) {
    let mut supplied = trailers::Supplied::default();
    let text = trailers::when(time, &mut supplied, time_field, tz_field);
    let (secs, offset) = text.split_once(' ').unwrap_or((text.as_str(), "+0000"));
    let seconds = secs.parse().unwrap_or(0);
    let sign: i16 = if offset.starts_with('-') { -1 } else { 1 };
    let digits = offset.get(1..).unwrap_or("0000");
    let hh: i16 = digits.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mm: i16 = digits.get(2..4).and_then(|s| s.parse().ok()).unwrap_or(0);
    (seconds, sign * (hh * 60 + mm))
}

fn identity_matches(result: Option<&Identity>, expected: &(String, String)) -> bool {
    let (name, email) = match result {
        Some(id) => (
            id.name.as_utf8().unwrap_or_default().to_string(),
            id.email
                .as_ref()
                .and_then(Text::as_utf8)
                .unwrap_or_default()
                .to_string(),
        ),
        None => (String::new(), String::new()),
    };
    name == expected.0 && email == expected.1
}

fn when_matches(result: Option<Time>, expected: (i64, i16)) -> bool {
    match result {
        Some(t) => t.seconds == expected.0 && t.offset_minutes.unwrap_or(0) == expected.1,
        None => expected == (0, 0),
    }
}

/// The message [`super::write_stream`] would have rendered for `atom`, byte for byte: the source
/// message, the trailing newline rule, the blank line, then the exact `Brygge-*` block (RFC 019 D-6 §2's
/// message row: "re-render it with the same functions and compare the bytes"). `digest_hex` and
/// `brygge_version` are the artifact's own, as `write_stream` used them.
fn expected_message(
    atom: &ChangeAtom,
    transforms: &[brygge_ir::TransformRecord],
    source_kind: &SourceKind,
    digest_hex: &str,
) -> Vec<u8> {
    let mut supplied = trailers::Supplied::default();
    let mut altered = trailers::Altered::default();
    // Re-derive the identity/time lines purely to collect the same Supplied/Altered marks
    // `write_stream` collected — the lines themselves are discarded; only the marks matter here.
    let _ = trailers::identity_line(
        atom.metadata.author.as_ref(),
        &mut supplied,
        &mut altered,
        "author",
    );
    let _ = trailers::when(
        atom.metadata.author_time,
        &mut supplied,
        "author-time",
        "author-time-zone",
    );
    match (&atom.metadata.committer, atom.metadata.commit_time) {
        (None, None) => {
            supplied.mark("committer");
            supplied.mark("commit-time");
        }
        (committer, commit_time) => {
            if committer.is_some() {
                let _ = trailers::identity_line(
                    committer.as_ref(),
                    &mut supplied,
                    &mut altered,
                    "committer",
                );
            } else {
                supplied.mark("committer");
            }
            if commit_time.is_some() {
                let _ = trailers::when(
                    commit_time,
                    &mut supplied,
                    "commit-time",
                    "commit-time-zone",
                );
            } else {
                supplied.mark("commit-time");
            }
        }
    }
    let source_label = trailers::source_kind_label(source_kind);
    let source_id = trailers::source_id_text(&atom.source.kind, &atom.source.atom_id);
    let mut lines = vec![
        format!(
            "Brygge-Encoded: brygge {} from {source_label}",
            env!("CARGO_PKG_VERSION")
        ),
        format!("Brygge-Source-Id: {source_id}"),
    ];
    if let Some(l) = supplied.trailer() {
        lines.push(l);
    }
    if let Some(l) = altered.trailer() {
        lines.push(l);
    }
    lines.extend(trailers::status_trailers(atom, transforms));
    lines.push(format!("Brygge-Artifact: sha256:{digest_hex}"));

    let mut body = atom
        .metadata
        .message
        .as_ref()
        .map(|t| t.bytes.clone())
        .unwrap_or_default();
    if !body.ends_with(b"\n") {
        body.push(b'\n');
    }
    body.push(b'\n');
    body.extend(lines.join("\n").into_bytes());
    body
}

#[allow(clippy::too_many_arguments)]
fn compare_matched_pair(
    artifact_atom: &ChangeAtom,
    result_atom: &ChangeAtom,
    a_state: &TreeState,
    r_state: &TreeState,
    artifact_to_result: &HashMap<AtomId, AtomId>,
    transforms: &[brygge_ir::TransformRecord],
    source_kind: &SourceKind,
    digest_hex: &str,
    diffs: &mut Vec<String>,
) {
    let label = format!(
        "{}{}",
        diff_source_label(&artifact_atom.source.kind, &artifact_atom.source.atom_id),
        diff_commit_suffix(&result_atom.source.atom_id)
    );

    let mapped_parents: Vec<Option<AtomId>> = artifact_atom
        .parents
        .iter()
        .map(|p| artifact_to_result.get(p).copied())
        .collect();
    let result_parents = &result_atom.parents;
    if mapped_parents.len() != result_parents.len()
        || mapped_parents
            .iter()
            .zip(result_parents)
            .any(|(expected, actual)| expected.as_ref() != Some(actual))
    {
        diffs.push(format!("{label}: parents differ"));
    }

    if a_state != r_state {
        diffs.push(format!("{label}: the resulting tree differs"));
    }

    let expected_author = expected_identity(artifact_atom.metadata.author.as_ref(), "author");
    if !identity_matches(result_atom.metadata.author.as_ref(), &expected_author) {
        diffs.push(format!("{label}: author differs"));
    }
    let expected_author_when = expected_when(
        artifact_atom.metadata.author_time,
        "author-time",
        "author-time-zone",
    );
    if !when_matches(result_atom.metadata.author_time, expected_author_when) {
        diffs.push(format!("{label}: author time differs"));
    }

    let (expected_committer, expected_commit_when) = match (
        &artifact_atom.metadata.committer,
        artifact_atom.metadata.commit_time,
    ) {
        (None, None) => (expected_author.clone(), expected_author_when),
        (committer, commit_time) => (
            if committer.is_some() {
                expected_identity(committer.as_ref(), "committer")
            } else {
                expected_author.clone()
            },
            if commit_time.is_some() {
                expected_when(commit_time, "commit-time", "commit-time-zone")
            } else {
                expected_author_when
            },
        ),
    };
    if !identity_matches(result_atom.metadata.committer.as_ref(), &expected_committer) {
        diffs.push(format!("{label}: committer differs"));
    }
    if !when_matches(result_atom.metadata.commit_time, expected_commit_when) {
        diffs.push(format!("{label}: commit time differs"));
    }

    let result_message = result_atom
        .metadata
        .message
        .as_ref()
        .map(|t| t.bytes.as_slice())
        .unwrap_or(&[]);
    let expected = expected_message(artifact_atom, transforms, source_kind, digest_hex);
    if result_message != expected.as_slice() {
        diffs.push(format!("{label}: message (with its trailers) differs"));
    }
}

/// The ref every artifact ref maps to, present in `result_refs` only — never the `result: &Ir`'s own ref
/// list, which omits `refs/brygge/other/*` and `HEAD` (RFC 019 D-6 §2: both are read from the raw
/// listing instead).
fn compare_refs(
    ir: &Ir,
    result: &Ir,
    result_refs: &[GitRef],
    artifact_to_result: &HashMap<AtomId, AtomId>,
    diffs: &mut Vec<String>,
) {
    let result_by_name: HashMap<&str, &GitRef> =
        result_refs.iter().map(|r| (r.name.as_str(), r)).collect();
    let result_annotated: HashMap<&str, &RefRecord> = result
        .refs
        .iter()
        .filter(|rf| rf.kind == RefKind::Tag && rf.annotation.is_some())
        .map(|rf| (rf.name.as_str(), rf))
        .collect();

    let mut expected_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for rf in &ir.refs {
        let path = super::git_ref_path(rf);
        expected_names.insert(path.clone());
        let Some(&found) = result_by_name.get(path.as_str()) else {
            diffs.push(format!("ref {path:?}: missing from the result"));
            continue;
        };
        let expected_target = artifact_to_result.get(&rf.target).copied();
        let matches_target = match (&found.target, &rf.annotation) {
            // An annotated tag's ref points at the tag object, not the commit — that object's own
            // identity (tagger/time/message) is checked separately below, not by the oid here.
            (GitRefTarget::Oid(_), Some(_)) => true,
            (GitRefTarget::Oid(oid), None) => commit_oid_matches(result, expected_target, oid),
            (GitRefTarget::Symbolic(_), _) => false,
        };
        if !matches_target {
            diffs.push(format!("ref {path:?}: points at a different commit"));
        }
        // `result_annotated` is keyed by the bare ref name (`RefRecord::name`'s own convention), never
        // the full `refs/tags/<name>` path `path` holds — looking it up by `path` here always missed,
        // the real bug a level-2 test (a real annotated tag, untampered) caught.
        if let (Some(ann), Some(found_rf)) =
            (&rf.annotation, result_annotated.get(rf.name.as_str()))
        {
            let expected_tagger = expected_identity(ann.tagger.as_ref(), "tagger");
            #[allow(clippy::expect_used)]
            let found_ann = found_rf
                .annotation
                .as_ref()
                .expect("filtered by annotation.is_some()");
            if !identity_matches(found_ann.tagger.as_ref(), &expected_tagger) {
                diffs.push(format!("tag {path:?}: tagger differs"));
            }
            let expected_tag_when = expected_when(ann.time, "tag-time", "tag-time-zone");
            if !when_matches(found_ann.time, expected_tag_when) {
                diffs.push(format!("tag {path:?}: tag time differs"));
            }
        } else if rf.annotation.is_some() != result_annotated.contains_key(rf.name.as_str()) {
            diffs.push(format!("tag {path:?}: annotation presence differs"));
        }
    }

    for found in result_refs {
        if found.name == "HEAD" {
            continue;
        }
        if !expected_names.contains(&found.name) {
            diffs.push(format!(
                "ref {:?}: not named by the artifact (nothing extra)",
                found.name
            ));
        }
    }
}

/// Whether `oid` (a result ref's direct target) is the Git commit the result decoder assigned to
/// `expected_target` (the matched result atom), identified by its own `source.atom_id` (the real Git
/// SHA brygge-decode-git read).
fn commit_oid_matches(result: &Ir, expected_target: Option<AtomId>, oid: &[u8]) -> bool {
    let Some(expected_target) = expected_target else {
        return false;
    };
    result
        .atoms
        .iter()
        .find(|a| a.id == expected_target)
        .is_some_and(|a| a.source.atom_id == oid)
}

/// `HEAD`'s own check (RFC 019 D-6 §2): it must be a symbolic ref naming the default branch G1 chose.
fn compare_head(default_branch_ref: &str, result_refs: &[GitRef], diffs: &mut Vec<String>) {
    let Some(head) = result_refs.iter().find(|r| r.name == "HEAD") else {
        diffs.push("HEAD: not reported by the repository listing".to_string());
        return;
    };
    match &head.target {
        GitRefTarget::Symbolic(name) if name == default_branch_ref => {}
        GitRefTarget::Symbolic(name) => {
            diffs.push(format!("HEAD: names {name:?}, not {default_branch_ref:?}"));
        }
        GitRefTarget::Oid(_) => diffs.push("HEAD: detached, not symbolic".to_string()),
    }
}

/// The full comparison (RFC 019 D-6 §2): matching, then parents/tree/author/committer/message per
/// matched pair, then refs (names, kinds, targets, annotations) and `HEAD`. `digest` is the artifact's
/// own digest, as `write_stream` wrote it into every `Brygge-Artifact:` trailer. `default_branch_ref` is
/// the full ref name (`refs/heads/<name>`) G1 chose as the default branch.
pub(crate) fn compare_git_target(
    ir: &Ir,
    digest: &[u8; 32],
    result: &Ir,
    result_refs: &[GitRef],
    default_branch_ref: &str,
) -> Comparison {
    let mut seen: HashMap<&[u8], &ChangeAtom> = HashMap::new();
    for atom in &ir.atoms {
        if let Some(first) = seen.insert(&atom.source.atom_id, atom) {
            return Comparison::NotChecked(format!(
                "two atoms in the artifact share one source id: {} and {}",
                trailers::truncate_for_refusal(&trailers::source_id_text(
                    &ir.provenance.source.kind,
                    &first.source.atom_id
                )),
                trailers::truncate_for_refusal(&trailers::source_id_text(
                    &ir.provenance.source.kind,
                    &atom.source.atom_id
                )),
            ));
        }
    }

    let digest_hex = trailers::to_hex(digest);
    let mut diffs = Vec::new();
    compare_atoms(ir, result, &digest_hex, &mut diffs);

    // `artifact_to_result` is rebuilt here (cheaply, from the trailers already parsed once above) for
    // `compare_refs`'s own ref-target check — kept local to each function rather than threaded through,
    // since re-matching is O(atoms) and refs are always far fewer than atoms.
    let mut by_source_id: HashMap<Vec<u8>, AtomId> = HashMap::new();
    for atom in &ir.atoms {
        by_source_id.insert(atom.source.atom_id.clone(), atom.id);
    }
    let mut artifact_to_result: HashMap<AtomId, AtomId> = HashMap::new();
    for commit in &result.atoms {
        let message = commit
            .metadata
            .message
            .as_ref()
            .map(|t| t.bytes.as_slice())
            .unwrap_or(&[]);
        if let Some(value) = trailers::extract_trailer(message, "Brygge-Source-Id") {
            if let Some(bytes) = trailers::parse_source_id(&ir.provenance.source.kind, &value) {
                if let Some(&artifact_id) = by_source_id.get(&bytes) {
                    artifact_to_result.insert(artifact_id, commit.id);
                }
            }
        }
    }

    compare_refs(ir, result, result_refs, &artifact_to_result, &mut diffs);
    compare_head(default_branch_ref, result_refs, &mut diffs);

    if diffs.is_empty() {
        Comparison::Corresponds
    } else {
        Comparison::DoesNotCorrespond(diffs)
    }
}

#[cfg(test)]
mod tests;
