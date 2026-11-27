//! What a file really is, from its first bytes, and whether its extension
//! says the same. Phones and apps save files whose name lies: Instagram's
//! photos are WebP named `.jpg`, iPhones' HEIC photos get renamed `.jpg`,
//! a QuickTime video ends up as `.mp4`, a voice note as `.mp3`. The
//! computer then opens them with the wrong program or not at all, and O2
//! Cloud may reject them (`MED-1050`) or keep them under a misleading name.
//!
//! Only kinds that can be told apart reliably by their first bytes are
//! here. Documents other than PDF aren't: a `.docx`, an `.xlsx` and a
//! `.zip` all start the same.

use std::io::Read;
use std::path::{Path, PathBuf};

/// The kinds of file told apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    // Photos.
    Jpeg,
    Png,
    Gif,
    Webp,
    Heic,
    Avif,
    Bmp,
    Tiff,
    // Videos.
    Mp4,
    Mov,
    ThreeGp,
    Avi,
    Mkv,
    Webm,
    // Audio.
    Mp3,
    M4a,
    Wav,
    Flac,
    Ogg,
    // Documents.
    Pdf,
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
            Kind::Mp4 => "mp4",
            Kind::Mov => "mov",
            Kind::ThreeGp => "3gp",
            Kind::Avi => "avi",
            Kind::Mkv => "mkv",
            Kind::Webm => "webm",
            Kind::Mp3 => "mp3",
            Kind::M4a => "m4a",
            Kind::Wav => "wav",
            Kind::Flac => "flac",
            Kind::Ogg => "ogg",
            Kind::Pdf => "pdf",
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
            Kind::Mp4 => "MP4",
            Kind::Mov => "QuickTime (MOV)",
            Kind::ThreeGp => "3GP",
            Kind::Avi => "AVI",
            Kind::Mkv => "Matroska (MKV)",
            Kind::Webm => "WebM",
            Kind::Mp3 => "MP3",
            Kind::M4a => "M4A",
            Kind::Wav => "WAV",
            Kind::Flac => "FLAC",
            Kind::Ogg => "Ogg",
            Kind::Pdf => "PDF",
        }
    }

    /// What it is: "image", "video", "audio file" or "document".
    pub fn what(self) -> &'static str {
        match self {
            Kind::Jpeg
            | Kind::Png
            | Kind::Gif
            | Kind::Webp
            | Kind::Heic
            | Kind::Avif
            | Kind::Bmp
            | Kind::Tiff => "image",
            Kind::Mp4 | Kind::Mov | Kind::ThreeGp | Kind::Avi | Kind::Mkv | Kind::Webm => "video",
            Kind::Mp3 | Kind::M4a | Kind::Wav | Kind::Flac | Kind::Ogg => "audio file",
            Kind::Pdf => "document",
        }
    }

    /// Whether O2 Cloud shows a file with this kind's extension as a
    /// photo, video or song (checked live): `.avif`, `.bmp`, `.tif`,
    /// `.3gp`, `.flac` and `.ogg`/`.opus` are kept as plain files, with no
    /// preview, player or duration. A PDF is a plain file anyway.
    pub fn shown_by_o2(self) -> bool {
        !matches!(
            self,
            Kind::Avif | Kind::Bmp | Kind::Tiff | Kind::ThreeGp | Kind::Flac | Kind::Ogg
        )
    }

    /// "a JPEG image", "an AVIF image", "a QuickTime (MOV) video".
    pub fn described(self) -> String {
        // Said with a vowel sound first: "an AVIF", "an MP4".
        let an = matches!(
            self,
            Kind::Avif | Kind::Avi | Kind::Mp4 | Kind::Mp3 | Kind::M4a | Kind::Ogg
        );
        format!(
            "{} {} {}",
            if an { "an" } else { "a" },
            self.name(),
            self.what()
        )
    }

    /// The kind an extension stands for (any case), if it's one told apart.
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
            "mp4" | "m4v" => Kind::Mp4,
            "mov" | "qt" => Kind::Mov,
            "3gp" | "3g2" | "3gpp" => Kind::ThreeGp,
            "avi" => Kind::Avi,
            "mkv" => Kind::Mkv,
            "webm" => Kind::Webm,
            "mp3" => Kind::Mp3,
            "m4a" | "m4b" => Kind::M4a,
            "wav" => Kind::Wav,
            "flac" => Kind::Flac,
            "ogg" | "oga" | "opus" => Kind::Ogg,
            "pdf" => Kind::Pdf,
            _ => return None,
        })
    }
}

/// How many bytes `detect` looks at.
pub const HEAD: usize = 64;

/// What the first bytes of a file say it is, if a kind told apart.
pub fn detect(head: &[u8]) -> Option<Kind> {
    let at = |i: usize, b: &[u8]| head.get(i..i + b.len()) == Some(b);
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Kind::Jpeg);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Png);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(Kind::Gif);
    }
    if at(0, b"RIFF") {
        return match head.get(8..12) {
            Some(b"WEBP") => Some(Kind::Webp),
            Some(b"WAVE") => Some(Kind::Wav),
            Some(b"AVI ") => Some(Kind::Avi),
            _ => None,
        };
    }
    if head.starts_with(b"BM") && head.len() >= 14 {
        return Some(Kind::Bmp);
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        return Some(Kind::Tiff);
    }
    if head.starts_with(b"%PDF-") {
        return Some(Kind::Pdf);
    }
    if head.starts_with(b"fLaC") {
        return Some(Kind::Flac);
    }
    if head.starts_with(b"OggS") {
        return Some(Kind::Ogg);
    }
    // Matroska and WebM: an EBML header naming its document type.
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let has = |s: &[u8]| head.windows(s.len()).any(|w| w == s);
        return if has(b"webm") {
            Some(Kind::Webm)
        } else if has(b"matroska") {
            Some(Kind::Mkv)
        } else {
            None
        };
    }
    // MP3: an ID3 tag, or an MPEG audio layer III frame (not AAC's ADTS,
    // which shares the sync).
    if head.starts_with(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE6 == 0xE2) {
        return Some(Kind::Mp3);
    }
    // ISO base media: a `ftyp` box with a major brand and compatible ones.
    if head.len() >= 16 && at(4, b"ftyp") {
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let end = size.clamp(16, head.len());
        let major = &head[8..12];
        let (compatible, _) = head[16..end].as_chunks::<4>();
        let mut brands = vec![major];
        brands.extend(compatible.iter().map(|c| &c[..]));
        let has = |b: &[u8]| brands.contains(&b);
        if has(b"avif") || has(b"avis") {
            return Some(Kind::Avif);
        }
        let heif: [&[u8]; 8] = [
            b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
        ];
        if heif.iter().any(|b| has(b)) {
            return Some(Kind::Heic);
        }
        return match major {
            b"qt  " => Some(Kind::Mov),
            b"M4A " | b"M4B " => Some(Kind::M4a),
            m if m.starts_with(b"3gp") || m.starts_with(b"3g2") => Some(Kind::ThreeGp),
            b"isom" | b"iso2" | b"iso4" | b"iso5" | b"iso6" | b"mp41" | b"mp42" | b"avc1"
            | b"M4V " | b"MSNV" | b"dash" | b"f4v " => Some(Kind::Mp4),
            _ => None,
        };
    }
    None
}

/// A file whose extension doesn't match what it is: what it says it is,
/// what it is, and the name it should have.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub named_as: Kind,
    pub is: Kind,
    pub rename_to: PathBuf,
}

/// Whether `path` (of a kind told apart by its extension) is something
/// else. `None` if it says the truth, its extension isn't one of the
/// kinds, or its contents aren't a kind told apart (empty, damaged,
/// unknown: left alone).
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
    fn test_detect_images() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some(Kind::Jpeg));
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n\0\0"), Some(Kind::Png));
        assert_eq!(detect(b"GIF89a\0\0"), Some(Kind::Gif));
        assert_eq!(detect(b"RIFF\x10\0\0\0WEBPVP8 "), Some(Kind::Webp));
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
        assert_eq!(detect(b"II*\0\x08\0\0\0"), Some(Kind::Tiff));
        assert_eq!(detect(b"BM\0\0\0\0\0\0\0\0\0\0\0\0"), Some(Kind::Bmp));
        assert_eq!(detect(b""), None);
        assert_eq!(detect(&[0; 32]), None);
    }

    #[test]
    fn test_detect_video_audio_pdf() {
        assert_eq!(detect(&ftyp(b"isom", &[b"isom", b"mp42"])), Some(Kind::Mp4));
        assert_eq!(detect(&ftyp(b"mp42", &[b"isom"])), Some(Kind::Mp4));
        assert_eq!(detect(&ftyp(b"qt  ", &[b"qt  "])), Some(Kind::Mov));
        assert_eq!(
            detect(&ftyp(b"3gp4", &[b"isom", b"3gp4"])),
            Some(Kind::ThreeGp)
        );
        assert_eq!(detect(&ftyp(b"M4A ", &[b"M4A ", b"mp42"])), Some(Kind::M4a));
        assert_eq!(detect(b"RIFF\x10\0\0\0AVI LIST"), Some(Kind::Avi));
        assert_eq!(detect(b"RIFF\x10\0\0\0WAVEfmt "), Some(Kind::Wav));
        assert_eq!(
            detect(b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01\x42\x82\x84webm"),
            Some(Kind::Webm)
        );
        assert_eq!(
            detect(b"\x1a\x45\xdf\xa3\xa3\x42\x86\x81\x01\x42\x82\x88matroska"),
            Some(Kind::Mkv)
        );
        assert_eq!(detect(b"ID3\x04\0\0\0\0\0\0"), Some(Kind::Mp3));
        assert_eq!(detect(&[0xFF, 0xFB, 0x90, 0x64]), Some(Kind::Mp3));
        // AAC's ADTS shares the sync but isn't MP3.
        assert_eq!(detect(&[0xFF, 0xF1, 0x50, 0x80]), None);
        assert_eq!(detect(b"fLaC\0\0\0\x22"), Some(Kind::Flac));
        assert_eq!(detect(b"OggS\0\x02\0\0"), Some(Kind::Ogg));
        assert_eq!(detect(b"%PDF-1.7\n"), Some(Kind::Pdf));
        // A ZIP (or .docx) isn't told apart.
        assert_eq!(detect(b"PK\x03\x04\x14\0\0\0"), None);
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
        // A HEIC photo named .JPG; a BMP named .jpg gets its name too.
        let heic = ftyp(b"heic", &[b"mif1"]);
        let m = mismatch_of(Path::new("IMG_2.JPG"), Kind::Jpeg, &heic).unwrap();
        assert_eq!(m.rename_to, PathBuf::from("IMG_2.heic"));
        let m = mismatch_of(
            Path::new("a.jpg"),
            Kind::Jpeg,
            b"BM\0\0\0\0\0\0\0\0\0\0\0\0",
        )
        .unwrap();
        assert_eq!(m.rename_to, PathBuf::from("a.bmp"));
        // A QuickTime video named .mp4, an M4A named .mp3, an Opus note
        // named .m4a.
        let m = mismatch_of(Path::new("v.mp4"), Kind::Mp4, &ftyp(b"qt  ", &[b"qt  "])).unwrap();
        assert_eq!(m.rename_to, PathBuf::from("v.mov"));
        let m = mismatch_of(Path::new("s.mp3"), Kind::Mp3, &ftyp(b"M4A ", &[b"mp42"])).unwrap();
        assert_eq!(m.rename_to, PathBuf::from("s.m4a"));
        let m = mismatch_of(Path::new("n.m4a"), Kind::M4a, b"OggS\0\x02\0\0").unwrap();
        assert_eq!(m.rename_to, PathBuf::from("n.ogg"));
        // Same family under another of its names: left alone.
        assert_eq!(
            mismatch_of(Path::new("a.m4v"), Kind::Mp4, &ftyp(b"isom", &[])),
            None
        );
        assert_eq!(
            mismatch_of(Path::new("n.opus"), Kind::Ogg, b"OggS\0\x02\0\0"),
            None
        );
        assert_eq!(
            mismatch_of(Path::new("a.jpeg"), Kind::Jpeg, &[0xFF, 0xD8, 0xFF]),
            None
        );
        assert_eq!(mismatch_of(Path::new("a.heif"), Kind::Heic, &heic), None);
        // Nothing to tell: left alone.
        assert_eq!(mismatch_of(Path::new("a.jpg"), Kind::Jpeg, &[]), None);
        assert_eq!(mismatch_of(Path::new("a.mp4"), Kind::Mp4, &[0; 20]), None);
        assert_eq!(Kind::of_extension("JPEG"), Some(Kind::Jpeg));
        assert_eq!(Kind::of_extension("docx"), None);
    }

    #[test]
    fn test_described() {
        assert_eq!(Kind::Avif.described(), "an AVIF image");
        assert_eq!(Kind::Jpeg.described(), "a JPEG image");
        assert_eq!(Kind::Mov.described(), "a QuickTime (MOV) video");
        assert_eq!(Kind::Mp3.described(), "an MP3 audio file");
        assert!(!Kind::ThreeGp.shown_by_o2() && Kind::Mp4.shown_by_o2());
    }
}
