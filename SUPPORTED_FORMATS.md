# Supported PTiles format versions

This client's format-version policy is decoupled from client semver (the
client spans multiple `.ptiles` file kinds, each with its own version
number) -- see `~/.hermes/plans/ptiles-client-extraction-plan.md`, Addendum 2,
decisions 2-3.

**Every `.ptiles` file kind is versioned independently.** The version byte at
header offset 8 is scoped to that file's magic and nothing else. There is no
global "PTiles release version": `buildings_v8` being at 8/9 says nothing
about what version `signals` or `water` should be, and adding a new layer
never requires bumping any existing layer. A new file kind starts at 1.

The practical consequences:

- A proposal to "ship these layers at v8 so they match the release" is a
  category error -- there is no release-wide version to match.
- Readers gate on the (magic, version) *pair*. A reader that doesn't know a
  magic simply never fetches that file, so new layers are additive and need
  no coordination with existing readers.
- Version numbers may therefore be freely reused across kinds. Two files both
  reading "version 1" are not the same format and are not comparable.

`PtilesFile::open` fails closed: any magic/version pair not listed in the
table below is rejected with `FileError::UnsupportedVersion`, naming both the
version found and the versions this client supports. No forward
compatibility is assumed for an unlisted version -- it is not tried, it is
rejected outright.

The table is generated from `ptiles_core::SUPPORTED_FORMATS` (see
`core/src/versions.rs`) via `ptiles_core::supported_formats_table()`. A test
(`core/src/versions.rs::tests::doc_matches_generated_table` /
`core/tests/supported_formats_doc.rs`) asserts this file's generated section
matches that function's output verbatim, so the table and the code can't
drift apart.

`PTILESA` (admin, `US.admin.ptiles`) has a real sample inspected and is now
supported. The address layer (`{STATE}.address.ptiles`, magic `PTILESA2`) also
lands on this same 7-byte magic: the reference `write_header` packs `magic[:7]`,
so `PTILESA2` truncates to `PTILESA` on disk. Admin and address are therefore
distinguished by structure (admin has `block_count == 0` and `aux_length > 0`;
address uses a v2 merged-block index with `block_count > 0`) and by filename,
not by magic. `PTILESD` (the SPEC.md TIGER addr format) was never built, and
`PTILESU` (routing, planned per SPEC.md) has no sample -- both remain absent and
any file with one of those magics is rejected (empty `supported` list) until a
real sample is inspected.

Note: SPEC.md's "Schema version" row (line 71) lists business as magic
`PTILESI\x00` version 2. The real `TN.business.ptiles` file inspected for
this table has magic `PTILESB\x00` version 3 instead. This table follows the
real file, not the (apparently stale) doc.

<!-- BEGIN GENERATED SUPPORTED_FORMATS TABLE -->
| File kind | Magic | Supported versions | Notes |
| --- | --- | --- | --- |
| buildings_v8 | `PTILESF\x00` | 8, 9 | v8 from original build; height_m (flags2 0x10) is a u8 of 0.5 m steps that saturates at 127.5 m, and is published for 0.2%-92% of buildings depending on the state; v9 adds business_tag/opening_hours (flags2 0x20/0x40), skipped by v8 decoder |
| roads | `PTILESR\x00` | 2 | SPEC.md and real TN.roads.ptiles agree (v2) |
| business | `PTILESB\x00` | 3, 4 | v3: u32 record_len, i32 abs coords. v4: no record_len, sequential uid, i16 cell-relative coords, chain_count instead of emails/socials |
| water | `PTILESW\x00` | 1 | matches SPEC.md (v1) |
| places | `PTILESP\x00` | 1 | matches SPEC.md (v1) |
| parks | `PTILESN\x00` | 1 | matches SPEC.md (v1) |
| rail | `PTILEST\x00` | 1 | matches SPEC.md (v1) |
| trails | `PTILESH\x00` | 1 | {STATE}.trails_v1.ptiles as published. Header is byte-for-byte the same shape as rail's PTILEST v1 (7-byte magic + NUL, version, bbox, counts) and the record framing is the one core::trails decodes -- verified against the live TN file, not inferred from SPEC.md, which does not list this magic |
| admin_or_address | `PTILESA\x00` | 1 | US.admin.ptiles (real sample inspected) AND {STATE}.address.ptiles both land on 7-byte magic PTILESA v1 -- the address encoder's PTILESA2 truncates to PTILESA via write_header's magic[:7]. Disambiguated by structure (admin: block_count 0, aux_length>0) and filename, not magic |
| business_name_index | `PTILESX\x00` | 1 | sidecar {STATE}.business_name_index.ptiles from scripts/build_business_name_index.py; not in SPEC.md's file table, but matches the real bytes the reference builder produced from TN.business.ptiles during this task (magic PTILESX v1, no dict) |
| signals | `PTILESS\x00` | 1 | NEW -- {ST}.signals.ptiles, traffic stops/give_ways from OSM highway=* nodes |
| camera | `PTILESC\x00` | 1 | NEW -- {ST}.camera.ptiles, surveillance cameras / ALPR from OSM man_made=surveillance |
<!-- END GENERATED SUPPORTED_FORMATS TABLE -->
