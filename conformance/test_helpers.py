"""Unit tests for the byte-level helpers used to build/check the corpus."""

import struct
import unittest

from conformance import check_published
from conformance import slice as corpus_slice


def entry_v1(cell, offset, length, features=1):
    row = bytearray(corpus_slice.ENTRY_SIZE_V1)
    struct.pack_into("<Q", row, 0, cell)
    row[8:14] = offset.to_bytes(6, "little")
    row[14:17] = length.to_bytes(3, "little")
    struct.pack_into("<H", row, 17, features)
    return bytes(row)


def entry_v2(cell, offset, length, features=1, cell_index=0):
    row = bytearray(corpus_slice.ENTRY_SIZE_V2)
    struct.pack_into("<Q", row, 0, cell)
    row[8:24] = bytes(range(16))
    row[24:30] = (offset & ((1 << 48) - 1)).to_bytes(6, "little")
    row[30:32] = (length & 0xFFFF).to_bytes(2, "little")
    row[32] = (offset >> 48) & 0xFF
    row[33] = (length >> 16) & 0xFF
    struct.pack_into("<H", row, 34, features)
    struct.pack_into("<H", row, 36, cell_index)
    return bytes(row)


class HeaderTests(unittest.TestCase):
    def test_build_header_patches_named_fields_and_preserves_unknown_bytes(self):
        original = bytearray([0xA5] * corpus_slice.HEADER_SIZE)
        original[0:7] = b"PTILESR"
        updated = corpus_slice.build_header(
            original,
            {"version": 2, "feature_count": 123, "blocks_offset": 4096},
        )
        parsed = corpus_slice.parse_header(updated)
        self.assertEqual(parsed["magic"], b"PTILESR")
        self.assertEqual(parsed["version"], 2)
        self.assertEqual(parsed["feature_count"], 123)
        self.assertEqual(parsed["blocks_offset"], 4096)
        self.assertEqual(updated[100:], original[100:])


class EntryTests(unittest.TestCase):
    def test_v1_read_and_patch_round_trip_full_packed_widths(self):
        raw = entry_v1(0x87264D106FFFFFF, (1 << 48) - 2, (1 << 24) - 3, 65534)
        parsed = corpus_slice.read_entry(raw, 0, corpus_slice.ENTRY_SIZE_V1)
        self.assertEqual(parsed["h3_cell"], 0x87264D106FFFFFF)
        self.assertEqual(parsed["block_offset"], (1 << 48) - 2)
        self.assertEqual(parsed["block_length"], (1 << 24) - 3)
        self.assertEqual(parsed["feature_count"], 65534)

        patched = corpus_slice.patch_entry(raw, corpus_slice.ENTRY_SIZE_V1, 17, 23)
        self.assertEqual(corpus_slice.read_entry(patched, 0, 19)["block_offset"], 17)
        self.assertEqual(corpus_slice.read_entry(patched, 0, 19)["block_length"], 23)
        self.assertEqual(patched[0:8], raw[0:8])
        self.assertEqual(patched[17:19], raw[17:19])

    def test_v2_patch_preserves_bbox_feature_count_and_cell_index(self):
        raw = entry_v2(100, (1 << 55) + 9, (1 << 23) + 7, 42, 6)
        parsed = corpus_slice.read_entry(raw, 0, corpus_slice.ENTRY_SIZE_V2)
        self.assertEqual(parsed["block_offset"], (1 << 55) + 9)
        self.assertEqual(parsed["block_length"], (1 << 23) + 7)

        patched = corpus_slice.patch_entry(raw, 38, (1 << 50) + 3, (1 << 20) + 5)
        parsed = corpus_slice.read_entry(patched, 0, 38)
        self.assertEqual(parsed["block_offset"], (1 << 50) + 3)
        self.assertEqual(parsed["block_length"], (1 << 20) + 5)
        self.assertEqual(patched[8:24], raw[8:24])
        self.assertEqual(patched[34:38], raw[34:38])

        # The published checker is a second implementation; pin agreement.
        counted = b"\x02\x00\x00\x00" + patched
        self.assertEqual(
            check_published.entry_at(counted, 4, 38),
            (100, (1 << 50) + 3, (1 << 20) + 5),
        )

    def test_width_detection_uses_declared_width_then_structural_probe(self):
        v1 = entry_v1(10, 500, 20) + entry_v1(20, 520, 21)
        self.assertEqual(
            corpus_slice.detect_entry_size(v1, 2, 4 + len(v1)),
            (19, "DeclaredLength", 19),
        )
        self.assertEqual(
            corpus_slice.detect_entry_size(v1, 2, 4 + 2 * 42),
            (19, "Probed", 42),
        )

        # A v2 entry read at the v1 stride sees zero block length in its bbox.
        v2 = entry_v2(10, 500, 20) + entry_v2(20, 520, 21)
        self.assertEqual(
            corpus_slice.detect_entry_size(v2, 2, 4 + len(v2)),
            (38, "DeclaredLength", 38),
        )

    def test_structural_validation_rejects_zero_length_descending_and_truncated(self):
        self.assertFalse(corpus_slice.structurally_valid(entry_v1(10, 0, 0), 1, 19))
        descending = entry_v1(20, 100, 4) + entry_v1(10, 104, 4)
        self.assertFalse(corpus_slice.structurally_valid(descending, 2, 19))
        self.assertFalse(corpus_slice.structurally_valid(entry_v1(10, 100, 4), 2, 19))


class OffsetTests(unittest.TestCase):
    def test_all_offset_bases_resolve_to_the_expected_absolute_byte(self):
        entry = {"block_offset": 500}
        header = {"index_offset": 256, "blocks_offset": 298}
        self.assertEqual(
            corpus_slice.offset_base_of([entry], header, 2, 19),
            ("Absolute", 0),
        )
        self.assertEqual(corpus_slice.resolve(entry, header, "Absolute", 0), 500)

        relative = {"block_offset": 25}
        self.assertEqual(
            corpus_slice.offset_base_of([relative], header, 2, 19),
            ("Relative", 0),
        )
        self.assertEqual(corpus_slice.resolve(relative, header, "Relative", 0), 323)

        corrected_header = {"index_offset": 256, "blocks_offset": 340}
        self.assertEqual(
            corpus_slice.offset_base_of([entry], corrected_header, 2, 38),
            ("AbsoluteCorrected", 4),
        )
        self.assertEqual(
            corpus_slice.resolve(entry, corrected_header, "AbsoluteCorrected", 4),
            496,
        )


class CoarseIndexTests(unittest.TestCase):
    @staticmethod
    def coarse(samples, stride=256, entry_count=1000):
        out = bytearray(b"PTCI" + bytes([1, 0, 0, 0]))
        out += struct.pack("<III", stride, len(samples), entry_count)
        for cell, index in samples:
            out += struct.pack("<QI", cell, index)
        return bytes(out)

    def test_retarget_keeps_only_samples_inside_the_slice(self):
        aux = self.coarse([(10, 0), (20, 256), (30, 512)])
        got, description = corpus_slice.retarget_coarse_index(aux, 300)
        self.assertIn("2/3 samples", description)
        self.assertEqual(struct.unpack_from("<I", got, 12)[0], 2)
        self.assertEqual(struct.unpack_from("<I", got, 16)[0], 300)
        self.assertEqual(got[20:], aux[20:44])

    def test_retarget_drops_short_truncated_and_out_of_slice_tables(self):
        self.assertEqual(corpus_slice.retarget_coarse_index(b"PTCI", 10)[0], b"")
        truncated = self.coarse([(10, 0)])[:-1]
        self.assertEqual(corpus_slice.retarget_coarse_index(truncated, 10)[0], b"")
        outside = self.coarse([(10, 256)])
        self.assertEqual(corpus_slice.retarget_coarse_index(outside, 10)[0], b"")


if __name__ == "__main__":
    unittest.main()
