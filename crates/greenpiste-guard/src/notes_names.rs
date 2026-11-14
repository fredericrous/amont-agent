//! The signed-notes names greenpiste reads, in one place (ADR-0068).
//!
//! greengate writes its notes under `greengate-*` names; until the census
//! (plan 2026-11-12-greengate-signs-notes-under-green-names, Phase 5) the
//! `amont-*` names stay readable. A reader tries [`NAMES`] in order, so the
//! green set wins, and takes a note only when its header is the one of the
//! set whose ref it came from: the two sets never mix within a note.

/// One name set: the refs, the signing namespace and the headers that go
/// together.
pub struct NoteNames {
    /// The provenance notes ref (`refs/notes/...`).
    pub provenance_ref: &'static str,
    /// The attestation notes ref.
    pub attest_ref: &'static str,
    /// The gate-stamp notes ref name, as `git notes --ref` takes it.
    pub gate_ref: &'static str,
    /// The SSH namespace a provenance record is signed under.
    pub provenance_namespace: &'static str,
    /// The first line of a provenance record.
    pub provenance_header: &'static str,
    /// The first line of an attestation note.
    pub attest_header: &'static str,
    /// The first token of a gate stamp.
    pub gate_header: &'static str,
}

/// The names greengate writes.
pub const GREEN: NoteNames = NoteNames {
    provenance_ref: "refs/notes/greengate-provenance",
    attest_ref: "refs/notes/greengate-attest",
    gate_ref: "greengate-gate",
    provenance_namespace: "greengate-provenance",
    provenance_header: "greengate-provenance-v1",
    attest_header: "greengate-attest-v2",
    gate_header: "greengate-gate-v1",
};

/// The names amont wrote. LEGACY: remove at the census (Phase 5).
pub const LEGACY: NoteNames = NoteNames {
    provenance_ref: "refs/notes/amont-provenance",
    attest_ref: "refs/notes/amont-attest",
    gate_ref: "amont-gate",
    provenance_namespace: "amont-provenance",
    provenance_header: "amont-provenance-v1",
    attest_header: "amont-attest-v2",
    gate_header: "amont-gate-v1",
};

/// Read order: green first, the legacy set only when green has nothing.
pub const NAMES: [&NoteNames; 2] = [&GREEN, &LEGACY];

#[cfg(test)]
mod set_tests {
    use super::NAMES;

    /// Each set's provenance header, ref and namespace go together, so a
    /// record signed under one set is filed under the same set's ref.
    #[test]
    fn each_set_keeps_its_provenance_names_together() {
        for names in NAMES {
            assert_eq!(
                names.provenance_header,
                format!("{}-v1", names.provenance_namespace)
            );
            assert_eq!(
                names.provenance_ref,
                format!("refs/notes/{}", names.provenance_namespace)
            );
        }
    }
}
