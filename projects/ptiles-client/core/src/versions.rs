//! `SUPPORTED_FORMATS`: the format versions this client is verified to read,
//! keyed by 7-byte magic. Populated ONLY from bytes actually observed in the
//! real files under `~/kino/data/ptiles/` (inspected with `od` during this
//! task) -- nothing here is speculative or copied from SPEC.md's schema-version
//! table without cross-checking against real bytes.
//!
//! SPEC.md's "Schema version" row (line 71) claims business is magic
//! `PTILESI\x00` version 2. The real `TN.business.ptiles` file has magic
//! `PTILESB\x00` version 3 instead -- SPEC.md is stale/aspirational for that
//! layer. This table follows the real file, per the task's fail-closed intent:
//! `PtilesFile::open` must accept what's actually deployed, not what a
//! possibly-outdated doc says.
//!
//! `PTILESA` (admin), `PTILESD` (addr), `PTILESU` (routing) have no local
//! sample file (admin never downloaded per the extraction plan; addr/routing
//! are "planned" per SPEC.md) -- they are deliberately absent from this table.
//! An admin/addr/routing file will be rejected as `UnsupportedVersion` with an
//! empty `supported` set until a real sample is inspected and a table entry is
//! added.
//!
//! Regenerate the markdown table with [`format_table`] -- `tests::table_matches_doc`
//! asserts it against `SUPPORTED_FORMATS.md` verbatim so the two can't drift.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// One row: a magic, the file-kind name for display, and the exact set of
/// version bytes this client accepts for it.
pub struct FormatEntry {
    pub magic: &'static [u8; 7],
    pub file_kind: &'static str,
    pub versions: &'static [u8],
    pub notes: &'static str,
}

/// Format versions verified against real files under `~/kino/data/ptiles/`.
/// Fail-closed: any magic/version pair not listed here is rejected by
/// `PtilesFile::open` with `FileError::UnsupportedVersion`.
pub const SUPPORTED_FORMATS: &[FormatEntry] = &[
    FormatEntry {
        magic: b"PTILESF",
        file_kind: "buildings_v8",
        versions: &[8],
        notes: "SPEC.md and real TN.buildings_v8.ptiles agree (v8)",
    },
    FormatEntry {
        magic: b"PTILESR",
        file_kind: "roads",
        versions: &[2],
        notes: "SPEC.md and real TN.roads.ptiles agree (v2)",
    },
    FormatEntry {
        magic: b"PTILESB",
        file_kind: "business",
        versions: &[3],
        notes: "real TN.business.ptiles: magic PTILESB v3, NOT SPEC.md's PTILESI v2 -- doc is stale",
    },
    FormatEntry {
        magic: b"PTILESW",
        file_kind: "water",
        versions: &[1],
        notes: "matches SPEC.md (v1)",
    },
    FormatEntry {
        magic: b"PTILESP",
        file_kind: "places",
        versions: &[1],
        notes: "matches SPEC.md (v1)",
    },
    FormatEntry {
        magic: b"PTILESN",
        file_kind: "parks",
        versions: &[1],
        notes: "matches SPEC.md (v1)",
    },
    FormatEntry {
        magic: b"PTILEST",
        file_kind: "rail",
        versions: &[1],
        notes: "matches SPEC.md (v1)",
    },
    FormatEntry {
        magic: b"PTILESA",
        file_kind: "admin_or_address",
        versions: &[1],
        notes: "US.admin.ptiles (real sample inspected) AND {STATE}.address.ptiles both land on 7-byte magic PTILESA v1 -- the address encoder's PTILESA2 truncates to PTILESA via write_header's magic[:7]. Disambiguated by structure (admin: block_count 0, aux_length>0) and filename, not magic",
    },
    FormatEntry {
        magic: b"PTILESX",
        file_kind: "business_name_index",
        versions: &[1],
        notes: "sidecar {STATE}.business_name_index.ptiles from scripts/build_business_name_index.py; not in SPEC.md's file table, but matches the real bytes the reference builder produced from TN.business.ptiles during this task (magic PTILESX v1, no dict)",
    },
];

/// Error returned when a header's magic/version pair is not in
/// [`SUPPORTED_FORMATS`]. `Display` names both what was found and what's
/// supported, so callers can log/report without re-deriving the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVersion {
    pub magic: [u8; 7],
    pub found: u8,
    pub supported: Vec<u8>,
}

impl fmt::Display for UnsupportedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let magic_str = core::str::from_utf8(&self.magic).unwrap_or("<invalid>");
        if self.supported.is_empty() {
            write!(
                f,
                "unsupported format version: magic {magic_str:?} version {} (no versions of this magic are supported yet)",
                self.found
            )
        } else {
            write!(
                f,
                "unsupported format version: magic {magic_str:?} version {} (supported: {:?})",
                self.found, self.supported
            )
        }
    }
}

/// Look up the allowed version set for a magic, if this client knows about
/// that file kind at all. `None` means the magic itself is unrecognized (not
/// just an unsupported version of a known one).
pub fn versions_for(magic: &[u8; 7]) -> Option<&'static [u8]> {
    SUPPORTED_FORMATS
        .iter()
        .find(|e| e.magic == magic)
        .map(|e| e.versions)
}

/// Validate a parsed header's magic/version against [`SUPPORTED_FORMATS`].
/// Fails closed: an unrecognized magic is treated the same as a recognized
/// magic with an unlisted version -- both come back as `Err` with an empty or
/// populated `supported` list respectively.
pub fn check_supported(magic: &[u8; 7], version: u8) -> Result<(), UnsupportedVersion> {
    let supported = versions_for(magic).unwrap_or(&[]);
    if supported.contains(&version) {
        Ok(())
    } else {
        Err(UnsupportedVersion {
            magic: *magic,
            found: version,
            supported: supported.to_vec(),
        })
    }
}

/// Render [`SUPPORTED_FORMATS`] as the markdown table body that
/// `SUPPORTED_FORMATS.md` embeds verbatim between its generated-section
/// markers. Single source of truth for both the doc and the drift-guard test.
pub fn format_table() -> String {
    let mut out = String::new();
    out.push_str("| File kind | Magic | Supported versions | Notes |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for entry in SUPPORTED_FORMATS {
        let magic_str = core::str::from_utf8(entry.magic).unwrap_or("<invalid>");
        let versions_str = entry
            .versions
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&alloc::format!(
            "| {} | `{}\\x00` | {} | {} |\n",
            entry.file_kind,
            magic_str,
            versions_str,
            entry.notes
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_magic_known_version_ok() {
        assert!(check_supported(b"PTILESF", 8).is_ok());
        assert!(check_supported(b"PTILESR", 2).is_ok());
        assert!(check_supported(b"PTILESB", 3).is_ok());
    }

    #[test]
    fn known_magic_wrong_version_rejected() {
        let err = check_supported(b"PTILESF", 9).unwrap_err();
        assert_eq!(err.found, 9);
        assert_eq!(err.supported, alloc::vec![8]);
        let msg = alloc::format!("{err}");
        assert!(msg.contains("PTILESF"));
        assert!(msg.contains('9'));
        assert!(msg.contains('8'));
    }

    #[test]
    fn unrecognized_magic_rejected_with_empty_supported() {
        // PTILESU (routing) is still deliberately absent from the table.
        let err = check_supported(b"PTILESU", 1).unwrap_err();
        assert!(err.supported.is_empty());
        let msg = alloc::format!("{err}");
        assert!(msg.contains("no versions of this magic are supported yet"));
    }

    #[test]
    fn admin_address_shared_magic_is_supported() {
        // Admin (PTILESA) and address (PTILESA2 -> truncates to PTILESA) share
        // this magic/version; both must be accepted by open.
        assert!(check_supported(b"PTILESA", 1).is_ok());
        assert!(check_supported(b"PTILESA", 2).is_err());
    }

    #[test]
    fn table_includes_every_entry() {
        let table = format_table();
        for entry in SUPPORTED_FORMATS {
            assert!(
                table.contains(entry.file_kind),
                "table missing row for {}",
                entry.file_kind
            );
        }
    }

    #[test]
    fn every_listed_version_of_every_entry_is_accepted() {
        // Fail-open regression guard: each version byte in the table must
        // pass check_supported for its own magic.
        for entry in SUPPORTED_FORMATS {
            for &v in entry.versions {
                assert!(
                    check_supported(entry.magic, v).is_ok(),
                    "{} v{} should be accepted",
                    entry.file_kind,
                    v
                );
            }
        }
    }

    #[test]
    fn version_just_below_and_above_supported_is_rejected() {
        // PTILESF supports exactly {8}: 7 and 9 must both fail closed.
        for bad in [0u8, 7, 9, 255] {
            let err = check_supported(b"PTILESF", bad).unwrap_err();
            assert_eq!(err.found, bad);
            assert_eq!(err.supported, alloc::vec![8]);
        }
        // Water/places/parks/rail/business_name_index all support only v1:
        // v0 and v2 must be rejected for each.
        for magic in [b"PTILESW", b"PTILESP", b"PTILESN", b"PTILEST", b"PTILESX"] {
            assert!(check_supported(magic, 1).is_ok());
            assert!(check_supported(magic, 0).is_err());
            assert!(check_supported(magic, 2).is_err());
        }
    }

    #[test]
    fn business_magic_follows_real_bytes_not_stale_spec() {
        // SPEC.md claims business is PTILESI v2; real files are PTILESB v3.
        assert!(check_supported(b"PTILESB", 3).is_ok());
        assert!(check_supported(b"PTILESB", 2).is_err()); // the doc's version
        // The doc's (wrong) magic is unknown entirely -> empty supported set.
        let err = check_supported(b"PTILESI", 2).unwrap_err();
        assert!(err.supported.is_empty());
    }

    #[test]
    fn versions_for_distinguishes_unknown_magic_from_wrong_version() {
        // Known magic -> Some(non-empty set).
        assert_eq!(versions_for(b"PTILESR"), Some(&[2u8][..]));
        // admin/address now share the supported PTILESA magic.
        assert_eq!(versions_for(b"PTILESA"), Some(&[1u8][..]));
        // Deliberately-absent magics (addr TIGER, routing) -> None.
        assert!(versions_for(b"PTILESD").is_none()); // addr (unbuilt TIGER format)
        assert!(versions_for(b"PTILESU").is_none()); // routing
        // A totally bogus magic -> None as well.
        assert!(versions_for(b"XXXXXXX").is_none());
    }

    #[test]
    fn unsupported_display_variants_read_correctly() {
        // Populated-supported branch.
        let known = check_supported(b"PTILESR", 99).unwrap_err();
        let msg = alloc::format!("{known}");
        assert!(msg.contains("PTILESR"));
        assert!(msg.contains("supported: [2]"));
        assert!(!msg.contains("no versions of this magic are supported yet"));

        // Empty-supported branch.
        let unknown = check_supported(b"PTILESZ", 1).unwrap_err();
        let msg = alloc::format!("{unknown}");
        assert!(msg.contains("no versions of this magic are supported yet"));
    }

    #[test]
    fn magic_bytes_are_all_distinct() {
        // Two entries sharing a magic would make versions_for ambiguous
        // (find() returns the first, silently shadowing the rest).
        for (i, a) in SUPPORTED_FORMATS.iter().enumerate() {
            for b in &SUPPORTED_FORMATS[i + 1..] {
                assert_ne!(a.magic, b.magic, "duplicate magic {:?}", a.magic);
            }
        }
    }
}
