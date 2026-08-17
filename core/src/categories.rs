//! The category table a business pack carries about itself.
//!
//! A business record stores one byte, `category_idx`, and nothing else. The
//! builder numbers categories by how common they are *within one state's
//! build* (`cat_idx = {c: i + 1 for i, (c, _) in enumerate(sorted_cats[:254])}`
//! in `build_full_ptilesb.py`), and writes the labels to a separate
//! `{ST}.business_categories.json`. Three things follow, and all three were
//! measured rather than supposed:
//!
//! - The number is state-relative. Churches are index 1 in Tennessee because
//!   they are its commonest category; elsewhere 1 is something else.
//! - It is also *build*-relative. The flight category is 94 in the published
//!   `TN.business.ptiles` and 96 in a sidecar built from a different snapshot
//!   (980,499 places against 829,528 named records).
//! - Neither file records which build it came from, so pairing a pack with the
//!   wrong sidecar mislabels everything past the first divergence -- silently,
//!   since the first entries still agree. In that pair, index 94 reads as
//!   `Elementary School` when the records under it are flights.
//!
//! A pack that names its own categories cannot drift from a sidecar, because
//! there is nothing to pair it with. That is what this section is: about 6 KB
//! against a 54 MB file, and the difference between a client showing
//! `business:94` and showing a category.
//!
//! Layout, little-endian, written at the end of the file and pointed to by
//! `aux_offset`/`aux_length`:
//!
//! ```text
//! magic    4  b"PTCT"
//! version  1
//! build    1 + n   short stamp identifying the build
//! count    2
//! entry    1 index, 1 group, 1 label length, n label bytes
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::DecodeError;

/// Magic of the category table section.
pub const CATEGORY_AUX_MAGIC: &[u8; 4] = b"PTCT";

/// The byte a record carries when its category is known but did not fit.
///
/// Only 254 categories fit in the field. The rest of the tail used to be
/// written as 0 -- the same value as "no category at all" -- so 37% of
/// Tennessee's records read as uncategorised and nobody could say how many of
/// them had a category that simply ranked too low. Packs built since carry
/// 255 for those, and 0 means what it says again.
pub const CATEGORY_OTHER: u8 = 255;

/// Coarse families, in the order the builder writes their index.
///
/// The group is the part worth comparing across packs: two states will not
/// agree on a number for `Taqueria`, but both call it dining. Kept in step
/// with `ptiles/categories.py::GROUPS`.
pub const GROUPS: [&str; 10] = [
    "arts_and_entertainment",
    "business_and_professional_services",
    "community_and_government",
    "dining_and_drinking",
    "health_and_medicine",
    "landmarks_and_outdoors",
    "retail",
    "sports_and_recreation",
    "travel_and_transportation",
    "other",
];

/// One category: the byte a record stores, and what it means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    /// The value found in a record's `category_idx`.
    pub index: u8,
    /// Canonical snake_case leaf, e.g. `church`, `gas_station`.
    pub label: String,
    /// Index into [`GROUPS`]; out-of-range values read as `other`.
    pub group: u8,
}

impl Category {
    /// The coarse family this category belongs to.
    pub fn group_name(&self) -> &'static str {
        GROUPS.get(self.group as usize).copied().unwrap_or("other")
    }
}

/// A pack's own account of its categories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryTable {
    /// Short stamp identifying the build that wrote this pack.
    ///
    /// Derived from what the build produced, not from the clock, so two runs
    /// over the same input agree. A sidecar carrying a different stamp is from
    /// a different build and its numbering cannot be trusted against this file.
    pub build_id: String,
    pub categories: Vec<Category>,
}

impl CategoryTable {
    /// The category a record's byte refers to.
    pub fn get(&self, index: u8) -> Option<&Category> {
        // Linear: a table is at most 254 entries and this is not a hot path.
        self.categories.iter().find(|c| c.index == index)
    }

    /// The label for a record's byte, or `None` when the pack does not name it.
    ///
    /// Index 0 is not a category: it is what the builder writes for a record
    /// the source left uncategorised.
    pub fn label(&self, index: u8) -> Option<&str> {
        self.get(index).map(|c| c.label.as_str())
    }

    /// Every index carrying this canonical label.
    ///
    /// More than one, sometimes. The builder folds two vocabularies to one
    /// leaf, so a state holding both `Landmarks and Outdoors > Park` and a
    /// bare `park` ranks them separately and writes two indices with the same
    /// label. Anything asking "all the parks" has to match the label across
    /// indices rather than pick one, which is the whole reason this is a
    /// method and not a reverse map the caller builds and gets wrong.
    pub fn indices_for(&self, label: &str) -> Vec<u8> {
        self.categories
            .iter()
            .filter(|c| c.label == label)
            .map(|c| c.index)
            .collect()
    }

    /// Whether this record's category was known but ranked past the 254 the
    /// field holds.
    ///
    /// Worth asking separately from [`Self::label`], because "we know this is
    /// something, and not which" is a different answer from "the source said
    /// nothing" -- and in an older pack the two are indistinguishable, which
    /// is why they are separated at all.
    pub fn is_truncated(&self, index: u8) -> bool {
        index == CATEGORY_OTHER
    }
}

/// Parse the aux section of a business pack.
///
/// Returns `Ok(None)` for a pack written before this section existed, which is
/// every file published so far: the absence is normal and not an error.
pub fn parse_category_table(aux: &[u8]) -> Result<Option<CategoryTable>, DecodeError> {
    if aux.len() < 4 || &aux[..4] != CATEGORY_AUX_MAGIC {
        return Ok(None);
    }
    let mut at = 4usize;
    let version = *aux.get(at).ok_or(DecodeError::UnexpectedEof { offset: at, needed: 1 })?;
    at += 1;
    if version != 1 {
        // A newer table is not readable, and guessing at it would produce
        // labels rather than an error, which is the failure this whole section
        // exists to prevent.
        return Ok(None);
    }
    let stamp_len = *aux.get(at).ok_or(DecodeError::UnexpectedEof { offset: at, needed: 1 })? as usize;
    at += 1;
    let stamp = aux
        .get(at..at + stamp_len)
        .ok_or(DecodeError::UnexpectedEof { offset: at, needed: stamp_len })?;
    let build_id = core::str::from_utf8(stamp)
        .map_err(|_| DecodeError::UnexpectedEof { offset: at, needed: stamp_len })?
        .to_string();
    at += stamp_len;

    let count_bytes = aux
        .get(at..at + 2)
        .ok_or(DecodeError::UnexpectedEof { offset: at, needed: 2 })?;
    let count = u16::from_le_bytes([count_bytes[0], count_bytes[1]]) as usize;
    at += 2;

    let mut categories = Vec::with_capacity(count);
    for _ in 0..count {
        let index = *aux.get(at).ok_or(DecodeError::UnexpectedEof { offset: at, needed: 1 })?;
        let group = *aux
            .get(at + 1)
            .ok_or(DecodeError::UnexpectedEof { offset: at + 1, needed: 1 })?;
        let label_len = *aux
            .get(at + 2)
            .ok_or(DecodeError::UnexpectedEof { offset: at + 2, needed: 1 })? as usize;
        at += 3;
        let raw = aux
            .get(at..at + label_len)
            .ok_or(DecodeError::UnexpectedEof { offset: at, needed: label_len })?;
        at += label_len;
        // A label that is not valid UTF-8 is skipped rather than failing the
        // table: one bad string should not cost every other category its name.
        if let Ok(label) = core::str::from_utf8(raw) {
            categories.push(Category { index, label: label.to_string(), group });
        }
    }
    Ok(Some(CategoryTable { build_id, categories }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn table(entries: &[(u8, &str, u8)], stamp: &str) -> Vec<u8> {
        let mut out = CATEGORY_AUX_MAGIC.to_vec();
        out.push(1);
        out.push(stamp.len() as u8);
        out.extend_from_slice(stamp.as_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (index, label, group) in entries {
            out.push(*index);
            out.push(*group);
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out
    }

    #[test]
    fn a_pack_names_its_own_categories() {
        let bytes = table(&[(1, "church", 2), (94, "plane", 8)], "d7660c794a51");

        let parsed = parse_category_table(&bytes).unwrap().expect("a table");

        assert_eq!(parsed.build_id, "d7660c794a51");
        assert_eq!(parsed.label(1), Some("church"));
        assert_eq!(parsed.label(94), Some("plane"));
        assert_eq!(parsed.get(94).unwrap().group_name(), "travel_and_transportation");
    }

    /// Every pack published so far has no such section, and that is not a
    /// fault: a reader has to carry on with numbers alone.
    #[test]
    fn a_pack_without_a_table_is_not_an_error() {
        assert_eq!(parse_category_table(&[]).unwrap(), None);
        assert_eq!(parse_category_table(b"not a table at all").unwrap(), None);
    }

    /// The whole point: the pack's own numbering, not a sidecar's.
    #[test]
    fn the_same_index_can_mean_different_things_in_two_packs() {
        let tn = parse_category_table(&table(&[(94, "plane", 8)], "aaa"))
            .unwrap()
            .unwrap();
        let ga = parse_category_table(&table(&[(94, "elementary_school", 2)], "bbb"))
            .unwrap()
            .unwrap();

        assert_eq!(tn.label(94), Some("plane"));
        assert_eq!(ga.label(94), Some("elementary_school"));
        assert_ne!(tn.build_id, ga.build_id);
    }

    /// Two spellings of one thing rank separately and both keep the label.
    #[test]
    fn a_label_can_sit_at_more_than_one_index() {
        let parsed = parse_category_table(&table(
            &[(1, "park", 5), (2, "park", 5), (3, "gas_station", 8)],
            "x",
        ))
        .unwrap()
        .unwrap();

        assert_eq!(parsed.indices_for("park"), vec![1, 2]);
        assert_eq!(parsed.indices_for("gas_station"), vec![3]);
        assert!(parsed.indices_for("nothing_like_it").is_empty());
    }

    /// A tail category and an absent one are different answers.
    #[test]
    fn truncation_no_longer_looks_like_absence() {
        let parsed = parse_category_table(&table(
            &[(1, "church", 2), (CATEGORY_OTHER, "other", 9)],
            "x",
        ))
        .unwrap()
        .unwrap();

        assert!(parsed.is_truncated(CATEGORY_OTHER));
        assert!(!parsed.is_truncated(0), "0 is the source saying nothing");
        assert!(!parsed.is_truncated(1));
        // And it is named, or the byte resolves to nothing at all.
        assert_eq!(parsed.label(CATEGORY_OTHER), Some("other"));
    }

    #[test]
    fn an_unknown_index_has_no_label() {
        let parsed = parse_category_table(&table(&[(1, "church", 2)], "x"))
            .unwrap()
            .unwrap();

        assert_eq!(parsed.label(0), None, "0 is 'no category', never a category");
        assert_eq!(parsed.label(200), None);
    }

    /// A newer table is refused rather than guessed at.
    #[test]
    fn a_future_version_reads_as_absent() {
        let mut bytes = table(&[(1, "church", 2)], "x");
        bytes[4] = 2;

        assert_eq!(parse_category_table(&bytes).unwrap(), None);
    }

    #[test]
    fn a_truncated_table_errors_rather_than_inventing_entries() {
        let bytes = table(&[(1, "church", 2), (2, "diner", 3)], "x");

        assert!(parse_category_table(&bytes[..bytes.len() - 3]).is_err());
    }

    /// Bytes the builder actually wrote, parsed by the reader that has to
    /// agree with it.
    ///
    /// Produced by `scripts/build_full_ptilesb.py::category_aux` in the ptiles
    /// repo for a three-category Tennessee build; regenerate with:
    ///
    /// ```text
    /// python3 -c "import importlib.util,sys; \
    ///   spec=importlib.util.spec_from_file_location('b','scripts/build_full_ptilesb.py'); \
    ///   m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m); \
    ///   print(list(m.category_aux('TN', {...}, 829528)))"
    /// ```
    ///
    /// The two implementations are a writer and a reader of the same bytes in
    /// two languages, which is precisely the pair that drifts silently.
    #[test]
    fn the_builders_own_bytes_parse() {
        let written: [u8; 51] = [
            80, 84, 67, 84, 1, 12, 53, 101, 102, 100, 56, 48, 48, 54, 51, 50, 57, 51, 3, 0, 1, 2,
            6, 99, 104, 117, 114, 99, 104, 2, 8, 11, 103, 97, 115, 95, 115, 116, 97, 116, 105, 111,
            110, 94, 8, 5, 112, 108, 97, 110, 101,
        ];

        let parsed = parse_category_table(&written).unwrap().expect("a table");

        assert_eq!(parsed.build_id, "5efd80063293");
        assert_eq!(parsed.categories.len(), 3);
        // The path vocabulary reduced to its leaf, and placed by its root.
        assert_eq!(parsed.label(1), Some("church"));
        assert_eq!(parsed.get(1).unwrap().group_name(), "community_and_government");
        // The bare vocabulary, placed by hand in ptiles/categories.py.
        assert_eq!(parsed.label(2), Some("gas_station"));
        assert_eq!(parsed.get(2).unwrap().group_name(), "travel_and_transportation");
        // And the category this whole thread started with.
        assert_eq!(parsed.label(94), Some("plane"));
        assert_eq!(parsed.get(94).unwrap().group_name(), "travel_and_transportation");
    }

    #[test]
    fn a_group_byte_out_of_range_reads_as_other() {
        let parsed = parse_category_table(&table(&[(1, "church", 99)], "x"))
            .unwrap()
            .unwrap();

        assert_eq!(parsed.get(1).unwrap().group_name(), "other");
    }
}
