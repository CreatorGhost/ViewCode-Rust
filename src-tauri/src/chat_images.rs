//! Images an agent's reply refers to (`![alt](path)`, a screenshot it saved), read for the
//! transcript and the image viewer.
//!
//! The rule is ViewCode's media-file rule: a path may point anywhere on this computer, inside
//! or outside the project. A relative path is taken from the task's checkout; an absolute path
//! is used as written. Whatever it names must resolve, through any symlinks, to a regular
//! file whose own name carries an image extension, so a link called `shot.png` cannot show
//! `~/.ssh/id_ed25519` or a FIFO. The bytes only ever go to the local window as a `data:` URL
//! (the app CSP allows no `blob:` images); nothing is fetched from the network, and a network
//! share path is refused.
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{codecs::jpeg::JpegEncoder, ImageFormat, ImageReader, Limits};
use serde::Serialize;
use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

/// Largest file sent to the window as it is.
const INLINE_LIMIT: u64 = 8 * 1024 * 1024;
/// Largest raster file read at all; between the two limits it is scaled down first.
const SOURCE_LIMIT: u64 = 64 * 1024 * 1024;
/// Long edge of a scaled-down image.
const SCALED_EDGE: u32 = 2560;
/// Largest edge a scaled-down source may decode from.
const DECODE_EDGE: u32 = 16_000;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatImage {
    /// `data:image/...;base64,` for an `<img>`.
    pub url: String,
    pub name: String,
    /// The resolved file, for the viewer's caption and "Reveal".
    pub path: String,
    /// Whether the file was too large to send whole and was scaled down.
    pub scaled: bool,
}

/// The MIME type for a file name's image extension (ViewCode's list of previewable images).
pub fn image_mime(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// The file `source` names, resolved through symlinks. Refuses anything but a regular file
/// with an image extension.
pub fn resolve(root: &Path, source: &str) -> Result<(PathBuf, &'static str), String> {
    // A network share (`\\server\share`, `//server/share`) is not this computer: naming one must
    // not make the app contact it.
    if source.is_empty()
        || source.len() > 4096
        || source.chars().any(char::is_control)
        || source.starts_with("\\\\")
        || source.starts_with("//")
    {
        return Err("This image path is not valid.".into());
    }
    let requested = Path::new(source);
    let requested = if requested.is_absolute() { requested.to_path_buf() } else { root.join(requested) };
    let target = requested.canonicalize().map_err(|_| "Image unavailable. The file may have been moved or deleted.")?;
    // The resolved name decides the type: a link named like an image shows nothing else.
    let mime = image_mime(&target).ok_or("Only PNG, JPEG, GIF, WebP, AVIF, SVG and ICO images can be shown.")?;
    let metadata = std::fs::metadata(&target).map_err(|_| "Image unavailable. The file may have been moved or deleted.")?;
    if !metadata.is_file() {
        return Err("This is not an image file.".into());
    }
    Ok((target, mime))
}

fn read(file: File, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("This image changed while it was read.".into());
    }
    Ok(bytes)
}

/// A JPEG (or, with transparency, PNG) no longer than `SCALED_EDGE` on its long side.
fn scale_down(bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let format = image::guess_format(bytes).map_err(|_| "This image is too large to show.")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif) {
        return Err("This image is too large to show.".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(DECODE_EDGE);
    limits.max_image_height = Some(DECODE_EDGE);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| "This image is damaged or too large to show.")?;
    let small = image.thumbnail(SCALED_EDGE, SCALED_EDGE);
    let mut out = Vec::new();
    if small.color().has_alpha() {
        small.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|_| "Could not prepare this image.")?;
        Ok((out, "image/png"))
    } else {
        JpegEncoder::new_with_quality(&mut out, 88).encode_image(&small.to_rgb8()).map_err(|_| "Could not prepare this image.")?;
        Ok((out, "image/jpeg"))
    }
}

fn load_with(root: &Path, source: &str, inline_limit: u64, source_limit: u64) -> Result<ChatImage, String> {
    let (target, mime) = resolve(root, source)?;
    let file = File::open(&target).map_err(|_| "Image unavailable. The file may have been moved or deleted.")?;
    // Checked again on the open handle: the path could have been swapped since `resolve`.
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("This is not an image file.".into());
    }
    let size = metadata.len();
    if size > source_limit {
        return Err(format!("This image is larger than {} MB.", source_limit / 1024 / 1024));
    }
    let (bytes, mime, scaled) = if size <= inline_limit {
        (read(file, inline_limit)?, mime, false)
    } else {
        let (bytes, mime) = scale_down(&read(file, source_limit)?)?;
        (bytes, mime, true)
    };
    Ok(ChatImage {
        url: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
        name: target.file_name().and_then(|name| name.to_str()).unwrap_or("Image").to_owned(),
        path: target.to_string_lossy().into_owned(),
        scaled,
    })
}

/// The image `source` names, as a `data:` URL. See the module rule for what is allowed.
pub fn load(root: &Path, source: &str) -> Result<ChatImage, String> {
    load_with(root, source, INLINE_LIMIT, SOURCE_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pandacode-chat-images-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
    fn png() -> Vec<u8> {
        STANDARD.decode(PNG).unwrap()
    }

    #[test]
    fn a_project_relative_image_is_read_from_the_checkout() {
        let root = temp("inside");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/shot.png"), png()).unwrap();
        let image = load(&root, "docs/shot.png").unwrap();
        assert_eq!(image.url, format!("data:image/png;base64,{PNG}"));
        assert_eq!(image.name, "shot.png");
        assert_eq!(image.path, root.join("docs/shot.png").to_string_lossy());
        assert!(!image.scaled);
        // `./` and an upper-case extension are the same file type.
        std::fs::write(root.join("Photo.JPG"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        assert!(load(&root, "./Photo.JPG").unwrap().url.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn an_absolute_image_outside_the_project_is_shown_as_viewcode_does() {
        let root = temp("project");
        let outside = temp("outside");
        std::fs::write(outside.join("screen.webp"), [1, 2, 3]).unwrap();
        let path = outside.join("screen.webp");
        let image = load(&root, path.to_str().unwrap()).unwrap();
        assert_eq!(image.url, "data:image/webp;base64,AQID");
        assert_eq!(image.path, path.to_string_lossy());
        // A relative path that climbs out of the checkout names the same file.
        let sibling = format!("../{}/screen.webp", outside.file_name().unwrap().to_str().unwrap());
        assert_eq!(load(&root, &sibling).unwrap().path, path.to_string_lossy());
    }

    #[test]
    fn missing_files_folders_and_other_file_types_are_refused() {
        let root = temp("refused");
        assert!(load(&root, "gone.png").unwrap_err().contains("moved or deleted"));
        std::fs::write(root.join("notes.txt"), "secret").unwrap();
        assert!(load(&root, "notes.txt").unwrap_err().contains("Only PNG"));
        std::fs::write(root.join("noextension"), png()).unwrap();
        assert!(load(&root, "noextension").is_err());
        std::fs::create_dir(root.join("folder.png")).unwrap();
        assert!(load(&root, "folder.png").unwrap_err().contains("not an image file"));
        assert!(load(&root, "").is_err());
        assert!(load(&root, "//server/share/shot.png").unwrap_err().contains("not valid"));
        assert!(load(&root, "\\\\server\\share\\shot.png").unwrap_err().contains("not valid"));
        assert!(load(&root, "a\nb.png").is_err());
        assert!(load(&root, &format!("{}.png", "x".repeat(5000))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_judged_by_the_file_it_resolves_to() {
        let root = temp("links");
        let outside = temp("links-outside");
        std::fs::write(outside.join("key.pem"), "PRIVATE").unwrap();
        std::fs::write(outside.join("real.png"), png()).unwrap();
        // Named like an image, pointing at something else: refused, and its bytes never read.
        std::os::unix::fs::symlink(outside.join("key.pem"), root.join("shot.png")).unwrap();
        let error = load(&root, "shot.png").unwrap_err();
        assert!(error.contains("Only PNG"), "{error}");
        // A link that does lead to an image shows it, and reports where it really is.
        std::os::unix::fs::symlink(outside.join("real.png"), root.join("alias.png")).unwrap();
        assert_eq!(load(&root, "alias.png").unwrap().path, outside.join("real.png").to_string_lossy());
        // A dangling link is a missing file.
        std::os::unix::fs::symlink(outside.join("gone.png"), root.join("dangling.png")).unwrap();
        assert!(load(&root, "dangling.png").unwrap_err().contains("moved or deleted"));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_named_like_an_image_is_refused_without_blocking() {
        let root = temp("fifo");
        let fifo = root.join("pipe.png");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !status.is_ok_and(|status| status.success()) {
            return;
        }
        assert!(load(&root, "pipe.png").unwrap_err().contains("not an image file"));
    }

    #[test]
    fn a_large_raster_image_is_scaled_down_and_anything_past_the_cap_is_refused() {
        let root = temp("large");
        let picture = DynamicImage::ImageRgb8(RgbImage::from_fn(3000, 40, |x, _| image::Rgb([(x % 251) as u8, 9, 200])));
        picture.save(root.join("wide.png")).unwrap();
        let size = std::fs::metadata(root.join("wide.png")).unwrap().len();
        // Under the inline limit the file goes as it is.
        assert!(!load_with(&root, "wide.png", size, size * 2).unwrap().scaled);
        // Over it, a smaller JPEG is sent instead.
        let scaled = load_with(&root, "wide.png", size - 1, size).unwrap();
        assert!(scaled.scaled);
        let data = STANDARD.decode(scaled.url.strip_prefix("data:image/jpeg;base64,").unwrap()).unwrap();
        let small = image::load_from_memory(&data).unwrap();
        assert_eq!((small.width(), small.height()), (SCALED_EDGE, 34));
        // Past the source cap nothing is read.
        assert!(load_with(&root, "wide.png", 16, size - 1).unwrap_err().contains("larger than"));
        // A vector or unknown format cannot be scaled, so it is refused rather than sent whole.
        std::fs::write(root.join("big.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>".repeat(4)).unwrap();
        assert!(load_with(&root, "big.svg", 16, 4096).unwrap_err().contains("too large"));
    }

    #[test]
    fn transparent_images_stay_png_when_scaled() {
        let root = temp("alpha");
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([1, 2, 3, 0]))).save(root.join("clear.png")).unwrap();
        let scaled = load_with(&root, "clear.png", 1, 1 << 20).unwrap();
        assert!(scaled.scaled && scaled.url.starts_with("data:image/png;base64,"));
    }
}
