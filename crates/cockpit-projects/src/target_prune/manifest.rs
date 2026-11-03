//! Bounded recovery manifest; no new serialization dependency or lossy path conversion.
use super::refused;
use crate::Error;

pub(super) const MAX_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub path: Vec<u8>,
    pub identity: String,
    pub bytes: u64,
    pub directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub root: String,
    pub target: String,
    pub entries: Vec<Entry>,
    pub bytes: u64,
}

pub(super) fn encode(entries: &[Entry]) -> Result<Vec<u8>, Error> {
    let mut result = Vec::new();
    if entries.len() > MAX_ENTRIES {
        return Err(refused("target manifest entry limit reached"));
    }
    for entry in entries {
        for field in [entry.path.as_slice(), entry.identity.as_bytes()] {
            let length =
                u32::try_from(field.len()).map_err(|_| refused("manifest field too large"))?;
            if result.len().saturating_add(field.len()).saturating_add(13) > MAX_BYTES {
                return Err(refused("target manifest byte limit reached"));
            }
            result.extend_from_slice(&length.to_le_bytes());
            result.extend_from_slice(field);
        }
        result.extend_from_slice(&entry.bytes.to_le_bytes());
        result.push(u8::from(entry.directory));
    }
    Ok(result)
}

pub(super) fn decode(mut input: &[u8]) -> Result<Vec<Entry>, Error> {
    fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], Error> {
        let (head, tail) = input
            .split_at_checked(count)
            .ok_or_else(|| refused("truncated cleanup manifest"))?;
        *input = tail;
        Ok(head)
    }
    fn field(input: &mut &[u8]) -> Result<Vec<u8>, Error> {
        let length = u32::from_le_bytes(
            take(input, 4)?
                .try_into()
                .map_err(|_| refused("manifest length"))?,
        ) as usize;
        Ok(take(input, length)?.to_vec())
    }
    if input.len() > MAX_BYTES {
        return Err(refused("stored cleanup manifest exceeds bound"));
    }
    let mut entries = Vec::new();
    while !input.is_empty() {
        if entries.len() == MAX_ENTRIES {
            return Err(refused("stored cleanup manifest exceeds entry bound"));
        }
        let path = field(&mut input)?;
        if path.contains(&0)
            || (!path.is_empty()
                && path
                    .split(|byte| *byte == b'/')
                    .any(|p| p.is_empty() || p == b"." || p == b".."))
        {
            return Err(refused("invalid relative cleanup entry"));
        }
        let identity = String::from_utf8(field(&mut input)?)
            .map_err(|_| refused("invalid cleanup identity"))?;
        let bytes = u64::from_le_bytes(
            take(&mut input, 8)?
                .try_into()
                .map_err(|_| refused("manifest bytes"))?,
        );
        let directory = match take(&mut input, 1)? {
            [0] => false,
            [1] => true,
            _ => return Err(refused("invalid cleanup type")),
        };
        entries.push(Entry {
            path,
            identity,
            bytes,
            directory,
        });
    }
    validate_tree(&entries)?;
    Ok(entries)
}

fn validate_tree(entries: &[Entry]) -> Result<(), Error> {
    let invalid = || refused("invalid cleanup manifest tree or accounting");
    if !entries
        .first()
        .is_some_and(|entry| entry.path.is_empty() && entry.directory)
    {
        return Err(invalid());
    }
    let mut bytes = 0_u64;
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 && entries[index - 1].path >= entry.path {
            return Err(invalid());
        }
        if entry.directory && entry.bytes != 0
            || !entry
                .identity
                .starts_with(if entry.directory { "d:" } else { "f:" })
        {
            return Err(invalid());
        }
        bytes = bytes.checked_add(entry.bytes).ok_or_else(invalid)?;
        if entry.path.is_empty() {
            continue;
        }
        if entry.path.split(|b| *b == b'/').count() > 65
            || entry.path.split(|b| *b == b'/').any(|part| {
                matches!(
                    part,
                    b".git" | b".torlek" | b".cockpit" | b".codex" | b".claude"
                )
            })
        {
            return Err(invalid());
        }
        let parent = entry
            .path
            .iter()
            .rposition(|b| *b == b'/')
            .map_or(&b""[..], |end| &entry.path[..end]);
        let parent_index = entries[..index]
            .binary_search_by(|entry| entry.path.as_slice().cmp(parent))
            .map_err(|_| invalid())?;
        if !entries[parent_index].directory {
            return Err(invalid());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &[u8], directory: bool) -> Entry {
        Entry {
            path: path.to_vec(),
            directory,
            bytes: if directory { 0 } else { 3 },
            identity: if directory {
                "d:1:1:1:1"
            } else {
                "f:1:1:2:1:3:1:1:1:1"
            }
            .into(),
        }
    }

    #[test]
    fn cleanup_manifest_rejects_unsafe_tree_structure_before_effects() {
        let root = entry(b"", true);
        for entries in [
            vec![],
            vec![entry(b"a", false)],
            vec![entry(b"", false)],
            vec![root.clone(), entry(b"b", false), entry(b"a", false)],
            vec![root.clone(), entry(b"a", false), entry(b"a", false)],
            vec![root.clone(), entry(b"a/b", false)],
            vec![root.clone(), entry(b"a", false), entry(b"a/b", false)],
            vec![root.clone(), entry(b".git", true)],
        ] {
            assert!(decode(&encode(&entries).unwrap()).is_err(), "{entries:?}");
        }
        let valid = vec![root, entry(b"sub", true), entry(b"sub/output", false)];
        assert_eq!(decode(&encode(&valid).unwrap()).unwrap(), valid);
    }

    #[test]
    fn cleanup_manifest_rejects_truncation_and_directory_byte_accounting() {
        let mut root = entry(b"", true);
        root.bytes = 99;
        assert!(decode(&encode(&[root]).unwrap()).is_err());
        let encoded = encode(&[entry(b"", true), entry(b"a", false)]).unwrap();
        // A complete root-only prefix is valid; all truncated field prefixes are not.
        let root_len = encode(&[entry(b"", true)]).unwrap().len();
        for count in 0..encoded.len() {
            if count != root_len {
                assert!(decode(&encoded[..count]).is_err(), "{count}");
            }
        }
    }
}
