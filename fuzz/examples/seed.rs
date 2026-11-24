//! Writes genuine objects as seed corpora: `cargo run --example seed`.
use paranoic_fuzz::fixture;
fn main() -> std::io::Result<()> {
    let fx = fixture(true);
    let put = |dir: &str, name: &str, bytes: &[u8]| -> std::io::Result<()> {
        std::fs::create_dir_all(format!("corpus/{dir}"))?;
        std::fs::write(format!("corpus/{dir}/{name}"), bytes)
    };
    put("v1_vault_header", "genuine", &fx.header)?;
    put("v1_entry_header", "file", &fx.file.1[..paranoic_formats::paranoic_v1::ENTRY_HEADER_LEN])?;
    put("v1_entry_header", "folder", &fx.folder.1)?;
    put("v1_decrypt_file", "padded", &fx.file.1)?;
    put("v1_decrypt_file", "plain", &fixture(false).file.1)?;
    put("v1_roundtrip", "small", &[0, 0, 5, 1, 2, 3, 4, 5, 6, 7, 8, 4, b'a', b'b', b'c', b'd'])?;
    put("v1_roundtrip", "chunk", &[1, 0x12, 1, 9, 9, 9, 9, 9, 9, 9, 9, 2, b'q', b'r'])?;
    Ok(())
}
