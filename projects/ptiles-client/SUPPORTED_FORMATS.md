# Supported PTiles format versions

This client's format-version policy is decoupled from client semver (the
client spans multiple `.ptiles` file kinds, each with its own version
number) -- see `~/.hermes/plans/ptiles-client-extraction-plan.md`, Addendum 2,
decisions 2-3.

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
| buildings_v8 | `PTILESF\x00` | 8 | SPEC.md and real TN.buildings_v8.ptiles agree (v8) |
| roads | `PTILESR\x00` | 2 | SPEC.md and real TN.roads.ptiles agree (v2) |
| business | `PTILESB\x00` | 3 | real TN.business.ptiles: magic PTILESB v3, NOT SPEC.md's PTILESI v2 -- doc is stale |
| water | `PTILESW\x00` | 1 | matches SPEC.md (v1) |
| places | `PTILESP\x00` | 1 | matches SPEC.md (v1) |
| parks | `PTILESN\x00` | 1 | matches SPEC.md (v1) |
| rail | `PTILEST\x00` | 1 | matches SPEC.md (v1) |
| admin_or_address | `PTILESA\x00` | 1 | US.admin.ptiles (real sample inspected) AND {STATE}.address.ptiles both land on 7-byte magic PTILESA v1 -- the address encoder's PTILESA2 truncates to PTILESA via write_header's magic[:7]. Disambiguated by structure (admin: block_count 0, aux_length>0) and filename, not magic |
| business_name_index | `PTILESX\x00` | 1 | sidecar {STATE}.business_name_index.ptiles from scripts/build_business_name_index.py; not in SPEC.md's file table, but matches the real bytes the reference builder produced from TN.business.ptiles during this task (magic PTILESX v1, no dict) |
<!-- END GENERATED SUPPORTED_FORMATS TABLE -->
