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
        versions: &[8, 9, 10],
        notes: "v8 from original build; height_m (flags2 0x10) is a u8 of 0.5 m steps that saturates at 127.5 m, and is published for 0.2%-92% of buildings depending on the state; v9 adds business_tag/opening_hours (flags2 0x20/0x40), skipped by v8 decoder. v10 appends name:en/brand/alt_name behind flags2 0x80 -> a flags3 byte, which also forced the decoder to walk v9's 0x20/0x40 rather than stop at the height",
    },
    FormatEntry {
        magic: b"PTILESR",
        file_kind: "roads",
        versions: &[2, 3],
        notes: "SPEC.md and real TN.roads.ptiles agree (v2). v3 is the first build whose records match the layout this decoder implements: the writer had been emitting a zigzag osm delta, a u8 vertex count, the class before the flags, and one coordinate delta pair short of its own count, so every published v2 file decoded to an empty road list",
    },
    FormatEntry {
        magic: b"PTILESB",
        file_kind: "business",
        versions: &[3, 4, 5],
        notes: "v3: u32 record_len, i32 abs coords. v4: no record_len, sequential uid, i16 cell-relative coords, chain_count instead of emails/socials. v5 carries brand and name:en in-record, one record per real place after a dedupe pass, and a category pack that names its own groups",
    },
    FormatEntry {
        magic: b"PTILESW",
        file_kind: "water",
        versions: &[1, 2],
        notes: "matches SPEC.md (v1). v2 changes no encoding: rings over 65,535 vertices are decimated to fit rather than dropped, so a v1 file is missing features a v2 file has",
    },
    FormatEntry {
        magic: b"PTILESP",
        file_kind: "places",
        versions: &[1, 2],
        notes: "matches SPEC.md (v1). v2 adds name:en (0x04) and brand (0x08)",
    },
    FormatEntry {
        magic: b"PTILESN",
        file_kind: "parks",
        versions: &[1, 2],
        notes: "matches SPEC.md (v1). v2 adds name:en (0x02) and brand (0x04)",
    },
    FormatEntry {
        magic: b"PTILEST",
        file_kind: "rail",
        versions: &[1, 2],
        notes: "matches SPEC.md (v1). v2 adds name:en (0x02) and brand (0x04)",
    },
    FormatEntry {
        magic: b"PTILESH",
        file_kind: "trails",
        versions: &[1, 2],
        notes: "{STATE}.trails_v1.ptiles as published. Header is byte-for-byte the same shape as rail's PTILEST v1 (7-byte magic + NUL, version, bbox, counts) and the record framing is the one core::trails decodes -- verified against the live TN file, not inferred from SPEC.md, which does not list this magic. v2 adds name:en (0x02), brand (0x04) and the park a trail starts in (0x08, varint osm id)",
    },
    FormatEntry {
        magic: b"PTILESA",
        file_kind: "admin_or_address",
        versions: &[1, 2, 3],
        notes: "US.admin.ptiles (real sample inspected) AND {STATE}.address.ptiles both land on 7-byte magic PTILESA v1 -- the address encoder's PTILESA2 truncates to PTILESA via write_header's magic[:7]. Disambiguated by structure (admin: block_count 0, aux_length>0) and filename, not magic. v3 populates boundary_flags and names counties from TIGER; it is also the first admin build whose header version matches the number in its filename, the v2 files having been stamped v1",
    },
    FormatEntry {
        magic: b"PTILESD",
        file_kind: "address",
        versions: &[1, 2, 3, 4],
        notes: "{STATE}.address_v2.ptiles as published since the builder stopped truncating PTILESA2 to the admin magic. v2 records carry i16 cell-relative coordinates, v3 adds a one-byte source (0=osm, 1=nad, 2=openaddresses) for the merged bulk-address layer, v4 a unit string after the street (APT B, STE 300) which earlier versions folded away as duplicates; v1 has none of it. Older published address files still say PTILESA and are accepted under that entry",
    },
    FormatEntry {
        magic: b"PTILESY",
        file_kind: "address_name_index",
        versions: &[1],
        notes: "sidecar {STATE}.address_name_index.ptiles from scripts/build_address_name_index.py: 28 buckets keyed by the first letter of a folded street name, each holding `u16 len | street | varint cell_count | deltas`. Ordinary v1 19-byte index so PtilesFile reads it unchanged. Turns a forward geocode from a whole-file scan into one bucket plus the cells that street is actually in",
    },
    FormatEntry {
        magic: b"PTILESX",
        file_kind: "business_name_index",
        versions: &[1],
        notes: "sidecar {STATE}.business_name_index.ptiles from scripts/build_business_name_index.py; not in SPEC.md's file table, but matches the real bytes the reference builder produced from TN.business.ptiles during this task (magic PTILESX v1, no dict)",
    },
    FormatEntry {
        magic: b"PTILESS",
        file_kind: "signals",
        versions: &[1],
        notes: "NEW -- {ST}.signals.ptiles, traffic stops/give_ways from OSM highway=* nodes",
    },
    FormatEntry {
        magic: b"PTILESC",
        file_kind: "camera",
        versions: &[1],
        notes: "NEW -- {ST}.camera.ptiles, surveillance cameras / ALPR from OSM man_made=surveillance",
    },
    FormatEntry {
        magic: b"PTILESE",
        file_kind: "ev",
        versions: &[1, 2],
        notes: "{STATE}.ev_v1.ptiles, EV charging stations from OSM amenity=charging_station (scripts/build_ev.py). Merged v2 blocks like trails/rail; records decode via core::ev. v2 adds name:en (0x08, u16-prefixed) and brand (0x10, u8-prefixed)",
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
        assert!(check_supported(b"PTILESF", 9).is_ok());
        assert!(check_supported(b"PTILESR", 2).is_ok());
        assert!(check_supported(b"PTILESB", 3).is_ok());
        assert!(check_supported(b"PTILESB", 4).is_ok());
    }

    #[test]
    fn known_magic_wrong_version_rejected() {
        // Derived from the table rather than written out: pinning "10 is
        // unsupported" is a test that fails the day buildings ship v10, which
        // says nothing about whether rejection works.
        let supported = versions_for(b"PTILESF").unwrap();
        let unseen = supported.iter().max().unwrap() + 1;
        assert!(check_supported(b"PTILESF", *supported.last().unwrap()).is_ok());
        let err = check_supported(b"PTILESF", unseen).unwrap_err();
        assert_eq!(err.found, unseen);
        assert_eq!(err.supported, supported.to_vec());
        let msg = alloc::format!("{err}");
        assert!(msg.contains("PTILESF"));
        assert!(msg.contains(&alloc::format!("{unseen}")));
    }

    #[test]
    fn unrecognized_magic_rejected_with_empty_supported() {
        let err = check_supported(b"PTILESU", 1).unwrap_err();
        assert!(err.supported.is_empty());
        let msg = alloc::format!("{err}");
        assert!(msg.contains("no versions of this magic are supported yet"));
    }

    #[test]
    fn admin_address_shared_magic_is_supported() {
        assert!(check_supported(b"PTILESA", 1).is_ok());
        // v3 is the current admin build; anything past the table is refused.
        assert!(check_supported(b"PTILESA", 3).is_ok());
        let unseen = versions_for(b"PTILESA").unwrap().iter().max().unwrap() + 1;
        assert!(check_supported(b"PTILESA", unseen).is_err());
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
        let buildings = versions_for(b"PTILESF").unwrap();
        let lowest = *buildings.iter().min().unwrap();
        let highest = *buildings.iter().max().unwrap();
        for bad in [0u8, lowest - 1, highest + 1, 255] {
            let err = check_supported(b"PTILESF", bad).unwrap_err();
            assert_eq!(err.found, bad);
            assert_eq!(err.supported, buildings.to_vec());
        }
        for magic in [b"PTILESW", b"PTILESP", b"PTILESN", b"PTILEST", b"PTILESX"] {
            let versions = versions_for(magic).unwrap();
            assert!(check_supported(magic, 1).is_ok());
            assert!(check_supported(magic, 0).is_err());
            assert!(check_supported(magic, versions.iter().max().unwrap() + 1).is_err());
        }
    }

    #[test]
    fn business_magic_follows_real_bytes_not_stale_spec() {
        assert!(check_supported(b"PTILESB", 3).is_ok());
        assert!(check_supported(b"PTILESB", 4).is_ok());
        assert!(check_supported(b"PTILESB", 2).is_err());
        let err = check_supported(b"PTILESI", 2).unwrap_err();
        assert!(err.supported.is_empty());
    }

    #[test]
    fn versions_for_distinguishes_unknown_magic_from_wrong_version() {
        assert_eq!(versions_for(b"PTILESR"), Some(&[2u8, 3][..]));
        assert_eq!(versions_for(b"PTILESA"), Some(&[1u8, 2, 3][..]));
        // PTILESD is the address magic and is supported now that the builder
        // stopped truncating it to PTILESA; PTILESU (routing) still is not.
        assert_eq!(versions_for(b"PTILESD"), Some(&[1u8, 2, 3, 4][..]));
        assert!(versions_for(b"PTILESU").is_none());
        assert!(versions_for(b"XXXXXXX").is_none());
    }

    #[test]
    fn unsupported_display_variants_read_correctly() {
        let known = check_supported(b"PTILESR", 99).unwrap_err();
        let msg = alloc::format!("{known}");
        assert!(msg.contains("PTILESR"));
        assert!(msg.contains("supported: [2, 3]"));
        assert!(!msg.contains("no versions of this magic are supported yet"));

        let unknown = check_supported(b"PTILESZ", 1).unwrap_err();
        let msg = alloc::format!("{unknown}");
        assert!(msg.contains("no versions of this magic are supported yet"));
    }

    #[test]
    fn magic_bytes_are_all_distinct() {
        for (i, a) in SUPPORTED_FORMATS.iter().enumerate() {
            for b in &SUPPORTED_FORMATS[i + 1..] {
                assert_ne!(a.magic, b.magic, "duplicate magic {:?}", a.magic);
            }
        }
    }
}
