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
    fn truncated_header_is_error() {
        let buf = [0u8; 100];
        assert!(Header::parse(&buf).is_err());
    }
}
