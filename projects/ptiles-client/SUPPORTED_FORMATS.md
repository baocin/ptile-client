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

Only magics with a real sample file inspected under `~/kino/data/ptiles/` are
listed. `PTILESA` (admin, `US.admin.ptiles`), `PTILESD` (addr, planned per
SPEC.md), and `PTILESU` (routing, planned per SPEC.md) have no local sample
and are therefore absent -- any file with one of those magics is rejected
today (empty `supported` list) until a real sample is inspected and a table
entry is added.

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
| business_name_index | `PTILESX\x00` | 1 | sidecar {STATE}.business_name_index.ptiles from scripts/build_business_name_index.py; not in SPEC.md's file table, but matches the real bytes the reference builder produced from TN.business.ptiles during this task (magic PTILESX v1, no dict) |
<!-- END GENERATED SUPPORTED_FORMATS TABLE -->
