//! Background loading of what the diff pane shows: a text diff, an image pair, or the
//! metadata of a binary file. Everything here runs on tokio workers.

use std::path::{Path, PathBuf};

use eframe::egui;
use facet_git::parse::FileDiff;
use facet_git::{
    diff_file, object_sizes, read_blob, CancellationToken, DiffRequest, DiffSource, Git, GitError,
    LfsPointer, RepoInfo,
};

use crate::filekind::{classify, extension, FileKind};
use crate::workspace::{BinaryInfo, ImageDiff, ImageSide};

/// Images are decoded only up to this file size…
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
/// …and this many pixels,
const MAX_IMAGE_PIXELS: u64 = 120_000_000;
/// and are scaled down to this edge length for display (GPU texture limits, memory).
const MAX_TEXTURE_EDGE: u32 = 4096;

pub enum DiffPayload {
    Text(FileDiff),
    Binary(BinaryInfo),
    Image(ImageDiff),
}

/// Where one side of a comparison comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// The file does not exist on this side (added / deleted).
    Missing,
    Blob(String),
    Worktree(PathBuf),
}

/// The old and new side for a diff request, given the entry's blobs.
pub fn sides(
    request: &DiffRequest,
    workdir: &Path,
    head_oid: Option<&str>,
    index_oid: Option<&str>,
) -> (Side, Side) {
    let blob = |oid: Option<&str>| oid.map_or(Side::Missing, |oid| Side::Blob(oid.to_owned()));
    let worktree = Side::Worktree(
        request
            .path
            .split('/')
            .fold(workdir.to_owned(), |path, part| path.join(part)),
    );
    match request.source {
        DiffSource::Staged => (blob(head_oid), blob(index_oid)),
        DiffSource::Unstaged => (blob(index_oid), worktree),
        DiffSource::Conflict => (blob(head_oid), worktree),
        DiffSource::Untracked => (Side::Missing, worktree),
    }
}

pub struct LoadContext {
    pub git: Git,
    pub info: RepoInfo,
    pub request: DiffRequest,
    pub head_oid: Option<String>,
    pub index_oid: Option<String>,
    pub ctx: egui::Context,
    pub cancel: CancellationToken,
}

/// Load whatever represents this change best. Known binary formats are never text-diffed.
pub async fn load(cx: &LoadContext) -> Result<DiffPayload, GitError> {
    let root = cx.info.root();
    let (old, new) = sides(
        &cx.request,
        root,
        cx.head_oid.as_deref(),
        cx.index_oid.as_deref(),
    );
    match classify(&cx.request.path) {
        FileKind::Image => {
            let (old_image, new_image) = tokio::join!(image_side(cx, &old), image_side(cx, &new));
            if old_image.is_some() || new_image.is_some() {
                return Ok(DiffPayload::Image(ImageDiff {
                    old: old_image,
                    new: new_image,
                }));
            }
            // Not decodable (corrupt, too large, or an LFS pointer without its content).
            Ok(DiffPayload::Binary(
                binary_info(cx, "Image", &old, &new).await?,
            ))
        }
        FileKind::Binary(description) => Ok(DiffPayload::Binary(
            binary_info(cx, description, &old, &new).await?,
        )),
        FileKind::Text => {
            let diff = diff_file(&cx.git, root, &cx.request, Some(&cx.cancel)).await?;
            if diff.binary {
                Ok(DiffPayload::Binary(
                    binary_info(cx, "Binary file", &old, &new).await?,
                ))
            } else {
                Ok(DiffPayload::Text(diff))
            }
        }
    }
}

/// The real bytes of one side, following a Git LFS pointer to its downloaded object when it
/// is already in the repository (nothing is ever fetched). `None`: missing or unavailable.
async fn side_bytes(cx: &LoadContext, side: &Side) -> Option<Vec<u8>> {
    let bytes = match side {
        Side::Missing => return None,
        Side::Worktree(path) => {
            let meta = tokio::fs::metadata(path).await.ok()?;
            if meta.len() > MAX_IMAGE_BYTES {
                return None;
            }
            tokio::fs::read(path).await.ok()?
        }
        Side::Blob(oid) => read_blob(&cx.git, cx.info.root(), oid, Some(&cx.cancel))
            .await
            .ok()?,
    };
    match LfsPointer::parse(&bytes) {
        Some(pointer) if pointer.size <= MAX_IMAGE_BYTES => {
            tokio::fs::read(pointer.local_object(&cx.info.git_dir))
                .await
                .ok()
        }
        Some(_) => None,
        None => Some(bytes),
    }
}

async fn image_side(cx: &LoadContext, side: &Side) -> Option<ImageSide> {
    let bytes = side_bytes(cx, side).await?;
    let path = cx.request.path.clone();
    let ctx = cx.ctx.clone();
    let label = format!("{side:?}");
    tokio::task::spawn_blocking(move || decode(&bytes, &path, &ctx, &label))
        .await
        .ok()
        .flatten()
}

/// Decode an image and upload it as a texture (scaled down if it is very large).
fn decode(bytes: &[u8], path: &str, ctx: &egui::Context, label: &str) -> Option<ImageSide> {
    // TGA has no signature, so the extension decides the format; content sniffing is the
    // fallback for files with a misleading extension.
    let format = extension(path)
        .and_then(image::ImageFormat::from_extension)
        .or_else(|| image::guess_format(bytes).ok())?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let decoded = match reader.decode() {
        Ok(decoded) => decoded,
        Err(_) => image::load_from_memory(bytes).ok()?,
    };
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return None;
    }
    let shown = if width.max(height) > MAX_TEXTURE_EDGE {
        decoded.resize(
            MAX_TEXTURE_EDGE,
            MAX_TEXTURE_EDGE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };
    let rgba = shown.to_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    let options = egui::TextureOptions {
        // Crisp pixels when zoomed in, smooth when scaled down.
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        ..Default::default()
    };
    let texture = ctx.load_texture(format!("diff:{path}:{label}"), image, options);
    Some(ImageSide {
        texture,
        width,
        height,
        bytes: bytes.len() as u64,
    })
}

/// Sizes of both sides of a binary change.
async fn binary_info(
    cx: &LoadContext,
    description: &str,
    old: &Side,
    new: &Side,
) -> Result<BinaryInfo, GitError> {
    let mut lfs = false;
    let old_size = side_size(cx, old, &mut lfs).await?;
    let new_size = side_size(cx, new, &mut lfs).await?;
    Ok(BinaryInfo {
        description: description.to_owned(),
        old_size,
        new_size,
        lfs,
    })
}

async fn side_size(cx: &LoadContext, side: &Side, lfs: &mut bool) -> Result<Option<u64>, GitError> {
    match side {
        Side::Missing => Ok(None),
        Side::Worktree(path) => Ok(tokio::fs::metadata(path).await.ok().map(|meta| meta.len())),
        Side::Blob(oid) => {
            let size = object_sizes(&cx.git, cx.info.root(), &[oid])
                .await?
                .into_iter()
                .next()
                .flatten();
            // A tiny blob for a binary format is probably an LFS pointer: report the size
            // of the real content instead of the pointer's.
            if size.is_some_and(|size| size <= 1024) {
                if let Ok(bytes) = read_blob(&cx.git, cx.info.root(), oid, Some(&cx.cancel)).await {
                    if let Some(pointer) = LfsPointer::parse(&bytes) {
                        *lfs = true;
                        return Ok(Some(pointer.size));
                    }
                }
            }
            Ok(size)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(source: DiffSource) -> DiffRequest {
        DiffRequest {
            path: "Content/Hero.png".into(),
            orig_path: None,
            source,
        }
    }

    #[test]
    fn sides_per_source() {
        let workdir = Path::new("repo");
        let file = Side::Worktree(Path::new("repo").join("Content").join("Hero.png"));
        let (head, index) = (Some("aaa"), Some("bbb"));
        assert_eq!(
            sides(&request(DiffSource::Staged), workdir, head, index),
            (Side::Blob("aaa".into()), Side::Blob("bbb".into()))
        );
        assert_eq!(
            sides(&request(DiffSource::Unstaged), workdir, head, index),
            (Side::Blob("bbb".into()), file.clone())
        );
        assert_eq!(
            sides(&request(DiffSource::Conflict), workdir, head, None),
            (Side::Blob("aaa".into()), file.clone())
        );
        assert_eq!(
            sides(&request(DiffSource::Untracked), workdir, None, None),
            (Side::Missing, file)
        );
        // A staged new file has nothing in HEAD; a staged deletion nothing in the index.
        assert_eq!(
            sides(&request(DiffSource::Staged), workdir, None, index).0,
            Side::Missing
        );
        assert_eq!(
            sides(&request(DiffSource::Staged), workdir, head, None).1,
            Side::Missing
        );
    }

    #[test]
    fn decoding_scales_huge_images_and_rejects_garbage() {
        let ctx = egui::Context::default();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let side = decode(&png, "a/b.png", &ctx, "test").unwrap();
        assert_eq!((side.width, side.height), (64, 32));
        assert_eq!(side.texture.size(), [64, 32]);
        assert_eq!(side.bytes, png.len() as u64);

        // Wrong extension: content sniffing still finds the PNG.
        assert!(decode(&png, "a/b.tga", &ctx, "test").is_some());
        assert!(decode(b"not an image at all", "a/b.png", &ctx, "test").is_none());

        // A TGA (no signature) decodes through its extension.
        let mut tga = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut tga), image::ImageFormat::Tga)
            .unwrap();
        assert_eq!(decode(&tga, "tex.tga", &ctx, "test").unwrap().width, 8);
    }
}
