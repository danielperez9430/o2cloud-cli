//! What an image file really is, from its first bytes, and whether its
//! extension says the same. Phones and apps save files whose name lies:
//! Instagram's photos are WebP named `.jpg`, iPhones' HEIC photos get
//! renamed `.jpg`. Windows then can't open them, and O2 Cloud may reject
//! them (`MED-1050`) or keep them under a name that misleads every viewer.

use std::io::Read;
use std::path::{Path, PathBuf};

/// The image formats told apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Gif,
    Webp,
    Heic,
    Avif,
    Bmp,
    Tiff,
}

impl Kind {
    /// The extension a file of this kind should have.
    pub fn extension(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpg",
            Kind::Png => "png",
            Kind::Gif => "gif",
            Kind::Webp => "webp",
            Kind::Heic => "heic",
            Kind::Avif => "avif",
            Kind::Bmp => "bmp",
            Kind::Tiff => "tif",
        }
    }

    /// Its usual name.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Jpeg => "JPEG",
            Kind::Png => "PNG",
            Kind::Gif => "GIF",
            Kind::Webp => "WebP",
            Kind::Heic => "HEIC",
            Kind::Avif => "AVIF",
            Kind::Bmp => "BMP",
            Kind::Tiff => "TIFF",
        }
    }

    /// The kind an extension stands for (any case), if it's an image one.
    pub fn of_extension(ext: &str) -> Option<Kind> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" => Kind::Jpeg,
            "png" => Kind::Png,
            "gif" => Kind::Gif,
            "webp" => Kind::Webp,
            "heic" | "heif" | "hif" => Kind::Heic,
            "avif" => Kind::Avif,
            "bmp" => Kind::Bmp,
            "tif" | "tiff" => Kind::Tiff,
            _ => return None,
        })
    }
}

/// How many bytes `detect` looks at.
pub const HEAD: usize = 64;

/// What the first bytes of a file say it is, if an image of a known kind.
pub fn detect(head: &[u8]) -> Option<Kind> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Kind::Jpeg);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Png);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(Kind::Gif);
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some(Kind::Webp);
    }
    if head.starts_with(b"BM") && head.len() >= 14 {
        return Some(Kind::Bmp);
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        return Some(Kind::Tiff);
    }
    // ISO base media (HEIF, AVIF): a `ftyp` box with a major brand and a
    // list of compatible ones. Video brands (`mp42`, `qt  `…) aren't
    // images and give `None`.
    if head.len() >= 16 && &head[4..8] == b"ftyp" {
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let end = size.clamp(16, head.len());
        let mut brands = vec![&head[8..12]];
        let (compatible, _) = head[16..end].as_chunks::<4>();
        brands.extend(compatible.iter().map(|c| &c[..]));
        let has = |b: &[u8]| brands.contains(&b);
        if has(b"avif") || has(b"avis") {
            return Some(Kind::Avif);
        }
        let heif = [
            &b"heic"[..],
            b"heix",
            b"hevc",
            b"hevx",
            b"heim",
            b"heis",
            b"mif1",
            b"msf1",
        ];
        if heif.iter().any(|b| has(b)) {
            return Some(Kind::Heic);
        }
    }
    None
}

/// An image whose extension doesn't match what it is: what it says it
/// is, what it is, and the name it should have.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub named_as: Kind,
    pub is: Kind,
    pub rename_to: PathBuf,
}

/// Whether `path` (an image by its extension) is something else. `None`
/// if it says the truth, isn't an image by name, or its contents aren't
/// a kind told apart (empty, damaged, unknown: left alone).
pub fn mismatch(path: &Path) -> std::io::Result<Option<Mismatch>> {
    let Some(named_as) = path
        .extension()
        .and_then(|e| Kind::of_extension(&e.to_string_lossy()))
    else {
        return Ok(None);
    };
    let mut head = Vec::with_capacity(HEAD);
    std::fs::File::open(path)?
        .take(HEAD as u64)
        .read_to_end(&mut head)?;
    Ok(mismatch_of(path, named_as, &head))
}

fn mismatch_of(path: &Path, named_as: Kind, head: &[u8]) -> Option<Mismatch> {
    let is = detect(head)?;
    (is != named_as).then(|| Mismatch {
        named_as,
        is,
        rename_to: path.with_extension(is.extension()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compatible.len();
        let mut b = (size as u32).to_be_bytes().to_vec();
        b.extend(b"ftyp");
        b.extend(major);
        b.extend([0, 0, 0, 0]);
        for c in compatible {
            b.extend(*c);
        }
        b.extend([0; 8]);
        b
    }

    #[test]
    fn test_detect() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some(Kind::Jpeg));
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n\0\0"), Some(Kind::Png));
        assert_eq!(detect(b"GIF89a\0\0"), Some(Kind::Gif));
        assert_eq!(detect(b"RIFF\x10\0\0\0WEBPVP8 "), Some(Kind::Webp));
        assert_eq!(detect(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(
            detect(&ftyp(b"heic", &[b"mif1", b"heic"])),
            Some(Kind::Heic)
        );
        assert_eq!(
            detect(&ftyp(b"mif1", &[b"mif1", b"miaf"])),
            Some(Kind::Heic)
        );
        assert_eq!(
            detect(&ftyp(b"avif", &[b"avif", b"mif1"])),
            Some(Kind::Avif)
        );
        assert_eq!(detect(&ftyp(b"mif1", &[b"avif"])), Some(Kind::Avif));
        // Videos aren't images.
        assert_eq!(detect(&ftyp(b"mp42", &[b"isom", b"mp42"])), None);
        assert_eq!(detect(&ftyp(b"qt  ", &[b"qt  "])), None);
        assert_eq!(detect(b"II*\0\x08\0\0\0"), Some(Kind::Tiff));
        assert_eq!(detect(b""), None);
        assert_eq!(detect(&[0; 32]), None);
    }

    #[test]
    fn test_mismatch() {
        let webp = b"RIFF\x10\0\0\0WEBPVP8 ";
        // Instagram's: WebP named .jpg, and renamed .heic.
        let m = mismatch_of(Path::new("dir/IMG_1.jpg"), Kind::Jpeg, webp).unwrap();
        assert_eq!((m.named_as, m.is), (Kind::Jpeg, Kind::Webp));
        assert_eq!(m.rename_to, PathBuf::from("dir/IMG_1.webp"));
        let m = mismatch_of(Path::new("IMG_1.heic"), Kind::Heic, webp).unwrap();
        assert_eq!(m.rename_to, PathBuf::from("IMG_1.webp"));
        // A HEIC photo named .JPG.
        let heic = ftyp(b"heic", &[b"mif1"]);
        let m = mismatch_of(Path::new("IMG_2.JPG"), Kind::Jpeg, &heic).unwrap();
        assert_eq!(m.rename_to, PathBuf::from("IMG_2.heic"));
        // Telling the truth, or nothing to tell: left alone.
        assert_eq!(
            mismatch_of(Path::new("a.jpeg"), Kind::Jpeg, &[0xFF, 0xD8, 0xFF]),
            None
        );
        assert_eq!(mismatch_of(Path::new("a.heif"), Kind::Heic, &heic), None);
        assert_eq!(mismatch_of(Path::new("a.jpg"), Kind::Jpeg, &[]), None);
        assert_eq!(Kind::of_extension("JPEG"), Some(Kind::Jpeg));
        assert_eq!(Kind::of_extension("mp4"), None);
    }
}
