//! PtilesHeader parse. Format: SPEC.md (~/kino/projects/ptiles/SPEC.md),
//! "Header (256 bytes)" section. Byte layout cross-checked against
//! `ptiles/codec.py::HEADER_STRUCT` ("<7sB B 3x f f f f Q I Q I Q I Q Q I 172x"):
//! magic(7) + null(1) + version(1) + pad(3) + min_lat/min_lon/max_lat/max_lon
//! (f32 each) + feature_count(u64) + block_count(u32) + dict_offset(u64) +
//! dict_length(u32) + index_offset(u64) + index_length(u32) +
//! blocks_offset(u64) + aux_offset(u64) + aux_length(u32) + reserved(172).

use crate::codec::DecodeError;

pub const HEADER_SIZE: usize = 256;

/// Parsed 256-byte PTiles file header.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Header {
    /// 7-byte magic prefix `PTILES` + layer byte (e.g. `b"PTILEST"` for rail).
    pub magic: [u8; 7],
    pub version: u8,
    pub min_lat: f32,
    pub min_lon: f32,
    pub max_lat: f32,
    pub max_lon: f32,
    pub feature_count: u64,
    pub block_count: u32,
    pub dict_offset: u64,
    pub dict_length: u32,
    pub index_offset: u64,
    pub index_length: u32,
    pub blocks_offset: u64,
    pub aux_offset: u64,
    pub aux_length: u32,
}

impl Header {
    /// Parse a 256-byte header from the start of a `.ptiles` file. Bounds-checked;
    /// truncated input yields `Err`, never a panic.
    pub fn parse(data: &[u8]) -> Result<Header, DecodeError> {
        if data.len() < HEADER_SIZE {
            return Err(DecodeError::UnexpectedEof {
                offset: 0,
                needed: HEADER_SIZE,
            });
        }

        let mut magic = [0u8; 7];
        magic.copy_from_slice(&data[0..7]);
        // byte 7 is the magic_null terminator (`\x00`), not otherwise used.
        let version = data[8];
        // bytes 9..12 are reserved alignment padding.

        let min_lat = f32::from_le_bytes(data[12..16].try_into().unwrap());
        let min_lon = f32::from_le_bytes(data[16..20].try_into().unwrap());
        let max_lat = f32::from_le_bytes(data[20..24].try_into().unwrap());
        let max_lon = f32::from_le_bytes(data[24..28].try_into().unwrap());
        let feature_count = u64::from_le_bytes(data[28..36].try_into().unwrap());
        let block_count = u32::from_le_bytes(data[36..40].try_into().unwrap());
        let dict_offset = u64::from_le_bytes(data[40..48].try_into().unwrap());
        let dict_length = u32::from_le_bytes(data[48..52].try_into().unwrap());
        let index_offset = u64::from_le_bytes(data[52..60].try_into().unwrap());
        let index_length = u32::from_le_bytes(data[60..64].try_into().unwrap());
        let blocks_offset = u64::from_le_bytes(data[64..72].try_into().unwrap());
        let aux_offset = u64::from_le_bytes(data[72..80].try_into().unwrap());
        let aux_length = u32::from_le_bytes(data[80..84].try_into().unwrap());

        Ok(Header {
            magic,
            version,
            min_lat,
            min_lon,
            max_lat,
            max_lon,
            feature_count,
            block_count,
            dict_offset,
            dict_length,
            index_offset,
            index_length,
            blocks_offset,
            aux_offset,
            aux_length,
        })
    }

    /// The 7-byte magic prefix as a UTF-8 str for display/comparison, e.g. `"PTILEST"`.
    pub fn magic_str(&self) -> &str {
        core::str::from_utf8(&self.magic).unwrap_or("<invalid>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_header(magic: &[u8; 7], version: u8) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(magic);
        buf[7] = 0;
        buf[8] = version;
        buf[12..16].copy_from_slice(&1.0f32.to_le_bytes());
        buf[36..40].copy_from_slice(&42u32.to_le_bytes());
        buf[64..72].copy_from_slice(&256u64.to_le_bytes());
        buf
    }

    // Populate every field with a distinct sentinel to catch offset mistakes.
    fn build_full_header() -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..7].copy_from_slice(b"PTILESB");
        buf[7] = 0;
        buf[8] = 8;
        buf[12..16].copy_from_slice(&(-36.5f32).to_le_bytes()); // min_lat
        buf[16..20].copy_from_slice(&(-120.25f32).to_le_bytes()); // min_lon
        buf[20..24].copy_from_slice(&37.75f32.to_le_bytes()); // max_lat
        buf[24..28].copy_from_slice(&(-119.0f32).to_le_bytes()); // max_lon
        buf[28..36].copy_from_slice(&123_456_789u64.to_le_bytes()); // feature_count
        buf[36..40].copy_from_slice(&4242u32.to_le_bytes()); // block_count
        buf[40..48].copy_from_slice(&1000u64.to_le_bytes()); // dict_offset
        buf[48..52].copy_from_slice(&200u32.to_le_bytes()); // dict_length
        buf[52..60].copy_from_slice(&2000u64.to_le_bytes()); // index_offset
        buf[60..64].copy_from_slice(&300u32.to_le_bytes()); // index_length
        buf[64..72].copy_from_slice(&4096u64.to_le_bytes()); // blocks_offset
        buf[72..80].copy_from_slice(&8000u64.to_le_bytes()); // aux_offset
        buf[80..84].copy_from_slice(&400u32.to_le_bytes()); // aux_length
        buf
    }

    #[test]
    fn parses_valid_header() {
        let buf = build_header(b"PTILEST", 1);
        let h = Header::parse(&buf).unwrap();
        assert_eq!(h.magic_str(), "PTILEST");
        assert_eq!(h.version, 1);
        assert_eq!(h.min_lat, 1.0);
        assert_eq!(h.block_count, 42);
        assert_eq!(h.blocks_offset, 256);
    }

    #[test]
    fn parses_all_fields_at_correct_offsets() {
        let buf = build_full_header();
        let h = Header::parse(&buf).unwrap();
        assert_eq!(&h.magic, b"PTILESB");
        assert_eq!(h.magic_str(), "PTILESB");
        assert_eq!(h.version, 8);
        assert_eq!(h.min_lat, -36.5);
        assert_eq!(h.min_lon, -120.25);
        assert_eq!(h.max_lat, 37.75);
        assert_eq!(h.max_lon, -119.0);
        assert_eq!(h.feature_count, 123_456_789);
        assert_eq!(h.block_count, 4242);
        assert_eq!(h.dict_offset, 1000);
        assert_eq!(h.dict_length, 200);
        assert_eq!(h.index_offset, 2000);
        assert_eq!(h.index_length, 300);
        assert_eq!(h.blocks_offset, 4096);
        assert_eq!(h.aux_offset, 8000);
        assert_eq!(h.aux_length, 400);
    }

    #[test]
    fn parse_at_exact_size_boundary_ok() {
        let buf = build_header(b"PTILESW", 3);
        assert_eq!(buf.len(), HEADER_SIZE);
        assert!(Header::parse(&buf).is_ok());
    }

    #[test]
    fn parse_ignores_trailing_bytes() {
        let hdr = build_full_header();
        let mut buf = hdr.to_vec();
        buf.extend_from_slice(&[0xab; 512]); // block data after header
        let h = Header::parse(&buf).unwrap();
        assert_eq!(h.version, 8);
        assert_eq!(h.aux_length, 400);
    }

    #[test]
    fn truncated_header_is_error() {
        let buf = [0u8; 100];
        assert!(matches!(
            Header::parse(&buf),
            Err(DecodeError::UnexpectedEof {
                offset: 0,
                needed: HEADER_SIZE
            })
        ));
    }

    #[test]
    fn one_byte_short_is_error() {
        let buf = [0u8; HEADER_SIZE - 1];
        assert!(Header::parse(&buf).is_err());
    }

    #[test]
    fn empty_input_is_error() {
        let empty: [u8; 0] = [];
        assert!(Header::parse(&empty).is_err());
    }

    #[test]
    fn magic_str_invalid_utf8_falls_back() {
        let mut buf = build_full_header();
        buf[0..7].copy_from_slice(&[0xff, 0xfe, 0xfd, 0x00, 0x01, 0x02, 0x03]);
        let h = Header::parse(&buf).unwrap();
        assert_eq!(h.magic_str(), "<invalid>");
    }

    #[test]
    fn version_zero_and_max_parse() {
        let h0 = Header::parse(&build_header(b"PTILESR", 0)).unwrap();
        assert_eq!(h0.version, 0);
        let h255 = Header::parse(&build_header(b"PTILESR", 255)).unwrap();
        assert_eq!(h255.version, 255);
    }
}
