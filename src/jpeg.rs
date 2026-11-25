//! JPEG colour profiles that O2 Cloud can't read.
//!
//! A JPEG carries its ICC colour profile in `APP2` segments of up to
//! 64 KB, so a larger profile is split into chunks, each labelled with
//! its number and the total ("chunk 2 of 6"). Some software labels every
//! chunk "1 of 1". Viewers cope, but O2 Cloud rejects the upload with
//! `MED-1000` (checked live in #24 with a 174 KB Kodak profile split in
//! six). Numbering the chunks properly, 12 bytes, makes it upload; the
//! image itself is untouched.

/// What is wrong with a JPEG's colour profile.
#[derive(Debug, PartialEq)]
pub enum ProfileProblem {
    /// The chunks are labelled wrongly, and put together in order they
    /// make exactly the profile it declares: `repair` fixes the labels.
    Mislabelled { chunks: usize },
    /// The chunks are labelled wrongly and don't make a whole profile, so
    /// the right order can't be known.
    Broken { chunks: usize },
}

const ICC_TAG: &[u8] = b"ICC_PROFILE\0";

/// The profile chunks of a JPEG: (offset of the segment's sequence-number
/// byte in `bytes`, sequence number, total, the chunk's data). `None` if
/// `bytes` isn't a JPEG whose header can be read up to the image data.
#[allow(clippy::type_complexity)]
fn icc_chunks(bytes: &[u8]) -> Option<Vec<(usize, u8, u8, &[u8])>> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut chunks = Vec::new();
    let mut i = 2;
    loop {
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        match marker {
            0xFF => {
                i += 1;
                continue;
            }
            // Start of scan or end of image: the header is over.
            0xDA | 0xD9 => return Some(chunks),
            // Markers without a length.
            0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            _ => {}
        }
        let len = u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]) as usize;
        let body = bytes.get(i + 4..i + 2 + len)?;
        if marker == 0xE2 && body.len() >= ICC_TAG.len() + 2 && body.starts_with(ICC_TAG) {
            let at = i + 4 + ICC_TAG.len();
            chunks.push((at, bytes[at], bytes[at + 1], &body[ICC_TAG.len() + 2..]));
        }
        i += 2 + len;
    }
}

/// What, if anything, is wrong with the colour profile of the JPEG in
/// `bytes`.
pub fn profile_problem(bytes: &[u8]) -> Option<ProfileProblem> {
    let chunks = icc_chunks(bytes)?;
    let n = chunks.len();
    let labelled_well = chunks
        .iter()
        .enumerate()
        .all(|(k, (_, seq, total, _))| *seq as usize == k + 1 && *total as usize == n);
    if n == 0 || labelled_well {
        return None;
    }
    // In file order, do the chunks make the profile its header declares?
    let profile: Vec<u8> = chunks.iter().flat_map(|c| c.3.iter().copied()).collect();
    let declared = profile
        .get(..4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let whole = n <= 255 && declared == Some(profile.len()) && profile.get(36..40) == Some(b"acsp");
    Some(if whole {
        ProfileProblem::Mislabelled { chunks: n }
    } else {
        ProfileProblem::Broken { chunks: n }
    })
}

/// `bytes` with the profile chunks labelled 1..n of n, if they are
/// `Mislabelled`. Nothing else changes.
pub fn repair(bytes: &[u8]) -> Option<Vec<u8>> {
    if !matches!(profile_problem(bytes)?, ProfileProblem::Mislabelled { .. }) {
        return None;
    }
    let chunks = icc_chunks(bytes)?;
    let n = chunks.len() as u8;
    let mut fixed = bytes.to_vec();
    for (k, (at, _, _, _)) in chunks.iter().enumerate() {
        fixed[*at] = k as u8 + 1;
        fixed[*at + 1] = n;
    }
    (profile_problem(&fixed).is_none()).then_some(fixed)
}

/// A one-line explanation of `problem`, for messages.
pub fn describe(problem: &ProfileProblem) -> String {
    match problem {
        ProfileProblem::Mislabelled { chunks } => format!(
            "its colour profile is split into {} chunks all labelled \"1 of 1\", which \
             O2 Cloud can't read; `o2cloud fix-jpeg <file> --apply` relabels them \
             without touching the image",
            chunks
        ),
        ProfileProblem::Broken { chunks } => format!(
            "its colour profile ({} chunks) is damaged, which O2 Cloud can't read; \
             saving the photo again from an image editor should fix it",
            chunks
        ),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A minimal JPEG header with an ICC profile of `size` bytes split in
    /// `chunk` byte chunks, each labelled by `label(k, n)`.
    pub fn jpeg_with_profile(
        size: usize,
        chunk: usize,
        label: impl Fn(usize, usize) -> (u8, u8),
    ) -> Vec<u8> {
        let mut profile = vec![0u8; size];
        profile[..4].copy_from_slice(&(size as u32).to_be_bytes());
        profile[36..40].copy_from_slice(b"acsp");
        for (k, b) in profile.iter_mut().enumerate().skip(40) {
            *b = (k % 251) as u8;
        }
        let parts: Vec<&[u8]> = profile.chunks(chunk).collect();
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        out.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        for (k, part) in parts.iter().enumerate() {
            let (seq, total) = label(k, parts.len());
            let len = 2 + ICC_TAG.len() + 2 + part.len();
            out.extend_from_slice(&[0xFF, 0xE2]);
            out.extend_from_slice(&(len as u16).to_be_bytes());
            out.extend_from_slice(ICC_TAG);
            out.extend_from_slice(&[seq, total]);
            out.extend_from_slice(part);
        }
        out.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0x12, 0x34, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn test_well_labelled_and_no_profile() {
        let good = jpeg_with_profile(100_000, 30_000, |k, n| (k as u8 + 1, n as u8));
        assert_eq!(profile_problem(&good), None);
        assert_eq!(repair(&good), None);
        let single = jpeg_with_profile(1_000, 30_000, |_, _| (1, 1));
        assert_eq!(profile_problem(&single), None);
        assert_eq!(profile_problem(b"not a jpeg"), None);
    }

    #[test]
    fn test_all_one_of_one_is_repaired() {
        let bad = jpeg_with_profile(173_912, 32_002, |_, _| (1, 1));
        assert_eq!(
            profile_problem(&bad),
            Some(ProfileProblem::Mislabelled { chunks: 6 })
        );
        let fixed = repair(&bad).unwrap();
        assert_eq!(profile_problem(&fixed), None);
        assert_eq!(fixed.len(), bad.len());
        let changed = bad.iter().zip(&fixed).filter(|(a, b)| a != b).count();
        assert!(changed <= 12, "{} bytes changed", changed);
        // The scan data is the same.
        assert_eq!(bad[bad.len() - 8..], fixed[fixed.len() - 8..]);
    }

    #[test]
    fn test_chunks_that_dont_make_a_profile_are_not_repaired() {
        let mut bad = jpeg_with_profile(100_000, 30_000, |_, _| (1, 1));
        // Claim a bigger profile than the chunks hold.
        let at = bad
            .windows(ICC_TAG.len())
            .position(|w| w == ICC_TAG)
            .unwrap()
            + ICC_TAG.len()
            + 2;
        bad[at..at + 4].copy_from_slice(&200_000u32.to_be_bytes());
        assert_eq!(
            profile_problem(&bad),
            Some(ProfileProblem::Broken { chunks: 4 })
        );
        assert_eq!(repair(&bad), None);
    }
}
