use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine;

const MAX_BYTES: u64 = 20 * 1024 * 1024;

pub struct Image {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

impl Image {
    pub fn data_uri(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.mime,
            base64::engine::general_purpose::STANDARD.encode(&self.bytes)
        )
    }

    pub fn extension(&self) -> &'static str {
        match self.mime {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            "image/bmp" => "bmp",
            _ => "png",
        }
    }
}

/// Resolve a provider-owned file path, refusing anything outside the allowed roots.
pub fn locate(source: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let requested = Path::new(source.trim());
    if source.trim().starts_with("data:") {
        return Err("Inline images have no file location".into());
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        roots
            .first()
            .ok_or("No image folder is available")?
            .join(requested)
    };
    let canonical = dunce::canonicalize(candidate).map_err(|_| "Image file is unavailable")?;
    if !roots
        .iter()
        .filter_map(|r| dunce::canonicalize(r).ok())
        .any(|r| canonical.starts_with(r))
    {
        return Err("Image is outside the workspace and provider output folders".into());
    }
    Ok(canonical)
}

/// Load an image from an inline data URI or a file the provider is allowed to expose.
pub fn load(source: &str, roots: &[PathBuf]) -> Result<Image, String> {
    let source = source.trim();
    if let Some(rest) = source.strip_prefix("data:") {
        return decode_data_uri(rest);
    }
    let canonical = locate(source, roots)?;
    let file = std::fs::File::open(canonical).map_err(|_| "Unable to open image")?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Unable to read image")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Image exceeds the 20 MB limit".into());
    }
    let mime = raster_mime(&bytes).ok_or("Unsupported image format")?;
    Ok(Image { bytes, mime })
}

fn decode_data_uri(rest: &str) -> Result<Image, String> {
    let (meta, data) = rest.split_once(',').ok_or("Malformed image data")?;
    let mime = meta
        .strip_suffix(";base64")
        .ok_or("Image data must be base64")?;
    let mime = match mime {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/webp" => "image/webp",
        "image/gif" => "image/gif",
        "image/bmp" => "image/bmp",
        _ => return Err("Unsupported image format".into()),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| "Malformed image data")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Image exceeds the 20 MB limit".into());
    }
    if raster_mime(&bytes) != Some(mime) {
        return Err("Image contents do not match the declared format".into());
    }
    Ok(Image { bytes, mime })
}

/// Validate a UI-supplied attachment (already base64) and normalise it.
pub fn attachment(mime: &str, data: &str) -> Result<hate_core::ImagePart, String> {
    let image = decode_data_uri(&format!("{mime};base64,{data}"))?;
    Ok(hate_core::ImagePart {
        mime: image.mime.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&image.bytes),
    })
}

pub fn default_name(source: &str, ext: &str) -> String {
    let from_path = Path::new(source.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && !source.trim().starts_with("data:"))
        .map_or_else(|| "generated-image".to_string(), str::to_string);
    format!("{from_path}.{ext}")
}

pub fn write_to(image: &Image, dest: &Path) -> Result<(), String> {
    std::fs::write(dest, &image.bytes).map_err(|e| format!("Couldn't save image: {e}"))
}

/// Only raster images inside the active workspace or provider-owned output folders.
pub fn read_image(path: &str, roots: &[PathBuf]) -> Result<String, String> {
    load(path, roots).map(|image| image.data_uri())
}

fn raster_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_scoped_and_raster_only() {
        let root = std::env::temp_dir().join(format!("hate-images-{}", uuid::Uuid::new_v4()));
        let allowed = root.join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::write(allowed.join("car.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(root.join("outside.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(allowed.join("bad.svg"), b"<svg></svg>").unwrap();
        let roots = [allowed];
        assert_eq!(
            read_image("car.png", &roots).unwrap(),
            "data:image/png;base64,iVBORw0KGgo="
        );
        assert!(read_image("../outside.png", &roots).is_err());
        assert!(read_image("bad.svg", &roots).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inline_images_are_validated_and_saved_where_asked() {
        let png = "data:image/png;base64,iVBORw0KGgo=";
        let image = load(png, &[]).unwrap();
        assert_eq!(image.mime, "image/png");
        assert_eq!(default_name(png, image.extension()), "generated-image.png");
        assert_eq!(default_name("C:\\images\\bmw_m6.jpg", "jpg"), "bmw_m6.jpg");
        let dir = std::env::temp_dir().join(format!("hate-save-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("chosen.png");
        write_to(&image, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"\x89PNG\r\n\x1a\n");
        std::fs::remove_dir_all(dir).unwrap();

        assert!(load("data:image/svg+xml;base64,PHN2Zz4=", &[]).is_err());
        assert!(load("data:image/png;base64,PHN2Zz4=", &[]).is_err());
        assert!(load("https://example.com/a.png", &[]).is_err());
    }
}
