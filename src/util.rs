// Shared utility helpers for upgrade and clone commands.

use std::cmp::Ordering;

/// Natural-sort comparator.  Numeric substrings are compared as integers
/// so "2.zip" < "10.zip" (not "10.zip" < "2.zip" as with ASCII sort).
pub fn nat_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a;
    let mut b = b;
    loop {
        match (a.is_empty(), b.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }

        let a_digit = a.starts_with(|c: char| c.is_ascii_digit());
        let b_digit = b.starts_with(|c: char| c.is_ascii_digit());

        if a_digit && b_digit {
            let an = a.find(|c: char| !c.is_ascii_digit()).unwrap_or(a.len());
            let bn = b.find(|c: char| !c.is_ascii_digit()).unwrap_or(b.len());
            let a_num: u64 = a[..an].parse().unwrap_or(0);
            let b_num: u64 = b[..bn].parse().unwrap_or(0);
            match a_num.cmp(&b_num) {
                Ordering::Equal => {
                    a = &a[an..];
                    b = &b[bn..];
                }
                other => return other,
            }
        } else {
            let ac = a.chars().next().unwrap();
            let bc = b.chars().next().unwrap();
            match ac.cmp(&bc) {
                Ordering::Equal => {
                    a = &a[ac.len_utf8()..];
                    b = &b[bc.len_utf8()..];
                }
                other => return other,
            }
        }
    }
}

/// Compute lowercase hex MD5 and SHA256 of a byte slice.
pub fn hash_bytes(data: &[u8]) -> (String, String) {
    use md5::Digest;
    let md5 = hex::encode(md5::Md5::digest(data));
    let sha256 = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(data));
    (md5, sha256)
}

/// Parse "59.4" → (59, 4).
pub fn parse_version(s: &str) -> Option<(u32, u32)> {
    let mut p = s.splitn(2, '.');
    let major: u32 = p.next()?.parse().ok()?;
    let minor: u32 = p.next()?.parse().ok()?;
    Some((major, minor))
}

pub fn version_string(v: (u32, u32)) -> String {
    format!("{}.{}", v.0, v.1)
}

/// Read a JSON file and deserialize it.
pub fn read_json_file<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> anyhow::Result<T> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("Bad JSON in {}: {e}", path.display()))
}

/// Serialize and write a JSON file.
pub fn write_json_file(path: &std::path::Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let raw = serde_json::to_string(value)?;
    std::fs::write(path, raw).map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nat_sort_zips() {
        let mut names = vec!["10.zip", "2.zip", "1.zip", "9.zip"];
        names.sort_by(|a, b| nat_cmp(a, b));
        assert_eq!(names, ["1.zip", "2.zip", "9.zip", "10.zip"]);
    }
    #[test]
    fn nat_sort_versions() {
        let mut vers = vec!["59.10", "59.2", "59.1", "59.9"];
        vers.sort_by(|a, b| nat_cmp(a, b));
        assert_eq!(vers, ["59.1", "59.2", "59.9", "59.10"]);
    }
}
