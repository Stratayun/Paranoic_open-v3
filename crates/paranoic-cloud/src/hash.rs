//! Microsoft QuickXorHash (OneDrive content hash): 160-bit, shift 11, length
//! XOR-ed into the last 8 bytes, base64. Used to compare local and remote
//! content without downloading. Verify against a known vector during checks.
use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Clone, Default)]
pub struct QuickXorHash { data: [u64; 3], shift: usize, length: u64 }
impl QuickXorHash {
    const WIDTH: usize = 160;
    const SHIFT: usize = 11;
    pub fn new() -> Self { Self::default() }
    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let index = self.shift / 64;
            let offset = self.shift % 64;
            let last = index == 2;
            let cell_bits = if last { 32 } else { 64 };
            self.data[index] ^= (byte as u64) << offset;
            if offset > cell_bits - 8 {
                let next = if last { 0 } else { index + 1 };
                self.data[next] ^= (byte as u64) >> (cell_bits - offset);
            }
            self.shift = (self.shift + Self::SHIFT) % Self::WIDTH;
        }
        self.length = self.length.wrapping_add(bytes.len() as u64);
    }
    pub fn finalize_base64(&self) -> String {
        let mut out = [0u8; 20];
        out[..8].copy_from_slice(&self.data[0].to_le_bytes());
        out[8..16].copy_from_slice(&self.data[1].to_le_bytes());
        out[16..].copy_from_slice(&self.data[2].to_le_bytes()[..4]);
        for (i, b) in self.length.to_le_bytes().iter().enumerate() { out[12 + i] ^= b; }
        STANDARD.encode(out)
    }
}
