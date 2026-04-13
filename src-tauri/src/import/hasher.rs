use crate::error::AppError;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

const BUF_SIZE: usize = 1 << 20; // 1 MB

/// Copy a file from `src` to `dst`, computing SHA-256 in a single pass.
/// Returns (hash, bytes_written).
pub fn copy_and_hash(dst: &Path, src: &Path) -> Result<([u8; 32], u64), AppError> {
    let mut src_file = File::open(src)?;
    let mut dst_file = File::create(dst)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut written: u64 = 0;

    loop {
        let n = src_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst_file.write_all(&buf[..n])?;
        written += n as u64;
    }
    dst_file.flush()?;

    let hash: [u8; 32] = hasher.finalize().into();
    Ok((hash, written))
}

/// Compute SHA-256 hash of a file.
pub fn hash_file(path: &Path) -> Result<[u8; 32], AppError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; BUF_SIZE];

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let hash: [u8; 32] = hasher.finalize().into();
    Ok(hash)
}

/// Format a hash as a lowercase hex string.
pub fn hash_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;

    #[test]
    fn test_copy_and_hash_matches_hash_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("dest.bin");

        // Write known content
        let mut f = File::create(&src).unwrap();
        f.write_all(b"hello world, this is a test file for hashing").unwrap();
        drop(f);

        let (copy_hash, written) = copy_and_hash(&dst, &src).unwrap();
        assert_eq!(written, 44);

        // Verify destination content matches source
        let src_content = fs::read(&src).unwrap();
        let dst_content = fs::read(&dst).unwrap();
        assert_eq!(src_content, dst_content);

        // Verify hash_file produces the same hash
        let file_hash = hash_file(&dst).unwrap();
        assert_eq!(copy_hash, file_hash);
    }

    #[test]
    fn test_hash_hex_format() {
        let hash = [0u8; 32];
        assert_eq!(
            hash_hex(&hash),
            "0000000000000000000000000000000000000000000000000000000000000000"
        );

        let mut hash2 = [0u8; 32];
        hash2[0] = 0xab;
        hash2[31] = 0xcd;
        let hex = hash_hex(&hash2);
        assert!(hex.starts_with("ab"));
        assert!(hex.ends_with("cd"));
    }

    #[test]
    fn test_large_file_hashing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("large.bin");
        let dst = dir.path().join("large_copy.bin");

        // Write 3 MB of data (larger than BUF_SIZE)
        let mut f = File::create(&src).unwrap();
        let chunk = vec![0x42u8; BUF_SIZE];
        for _ in 0..3 {
            f.write_all(&chunk).unwrap();
        }
        drop(f);

        let (copy_hash, written) = copy_and_hash(&dst, &src).unwrap();
        assert_eq!(written, 3 * BUF_SIZE as u64);

        let file_hash = hash_file(&dst).unwrap();
        assert_eq!(copy_hash, file_hash);
    }
}
