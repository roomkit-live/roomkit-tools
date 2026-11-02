//! Actual pixels, orientation, path boundaries and host validation.
use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use roomkit_tools_core::ToolError;
use roomkit_tools_image::{
    ImageMime, ImageWorkspace, ViewImageInput, ViewImageLimits, ViewImageTool,
};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

fn input(path: &str) -> ViewImageInput {
    ViewImageInput {
        path: path.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn transparent_pixels_revision_and_source_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let pixels = RgbaImage::from_fn(3, 2, |x, y| {
        Rgba([
            u8::try_from(x * 70).unwrap(),
            u8::try_from(y * 80).unwrap(),
            50,
            if x == 0 { 0 } else { 255 },
        ])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(pixels.clone())
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    std::fs::write(root.path().join("pixels.bin"), bytes.get_ref()).unwrap();
    let tool = ViewImageTool::new(root.path()).await.unwrap();
    let absolute = tool
        .view(
            input(root.path().join("pixels.bin").to_str().unwrap()),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = tool
        .view(input("pixels.bin"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(absolute, result);
    assert_eq!(result.data.path, "pixels.bin");
    assert!(result.text.starts_with("Image \"pixels.bin\": 3x2"));
    assert!(!result.text.contains(root.path().to_str().unwrap()));
    let png = STANDARD.decode(&result.data.image.base64).unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), pixels);
    assert_eq!(result.data.output_bytes, png.len());
    assert_eq!(
        result.data.revision,
        roomkit_tools_core::revision::sha256(&Sha256::digest(bytes.get_ref()))
    );
    assert_eq!(
        std::fs::read(root.path().join("pixels.bin")).unwrap(),
        bytes.into_inner()
    );
    assert!(!result.text.contains(&result.data.image.base64));
    assert!(!format!("{result:?}").contains(&result.data.image.base64));
    assert_eq!((result.data.width, result.data.height), (3, 2));
}

#[tokio::test]
async fn jpeg_exif_rotation_happens_before_resize() {
    let root = tempfile::tempdir().unwrap();
    let mut jpeg = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 20, Rgb([230, 10, 10])))
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .unwrap();
    // APP1/EXIF orientation=6 (90 degrees clockwise), little-endian TIFF.
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(exif);
    bytes.extend_from_slice(&jpeg.get_ref()[2..]);
    std::fs::write(root.path().join("rotated.jpg"), bytes).unwrap();
    let tool = ViewImageTool::new(root.path())
        .await
        .unwrap()
        .with_limits(ViewImageLimits {
            max_edge: 10,
            ..Default::default()
        })
        .unwrap();
    let result = tool
        .view(input("rotated.jpg"), &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert_eq!(result.source_mime_type, ImageMime::Jpeg);
    assert_eq!((result.original_width, result.original_height), (40, 20));
    assert_eq!((result.width, result.height), (5, 10));
    assert!(result.orientation_applied && result.resized);
    let png = image::load_from_memory(&STANDARD.decode(result.image.base64).unwrap())
        .unwrap()
        .to_rgb8();
    let pixel = png.get_pixel(2, 5);
    assert!(pixel[0] > 200 && pixel[1] < 30);
}

#[tokio::test]
async fn static_webp_is_detected_and_normalized() {
    let root = tempfile::tempdir().unwrap();
    let original = RgbImage::from_pixel(8, 4, Rgb([10, 80, 150]));
    let mut webp = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(original.clone())
        .write_to(&mut webp, ImageFormat::WebP)
        .unwrap();
    std::fs::write(root.path().join("image.dat"), webp.into_inner()).unwrap();
    let result = ViewImageTool::new(root.path())
        .await
        .unwrap()
        .view(input("image.dat"), &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert_eq!(result.source_mime_type, ImageMime::Webp);
    assert_eq!(result.image.mime_type, ImageMime::Png);
    assert_eq!(
        image::load_from_memory(&STANDARD.decode(result.image.base64).unwrap())
            .unwrap()
            .to_rgb8(),
        original
    );
}

#[tokio::test]
async fn outside_paths_are_refused_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("present.png"), b"irrelevant").unwrap();
    let tool = ViewImageTool::new(root.path()).await.unwrap();
    let workspace = ImageWorkspace::new(root.path()).await.unwrap();
    for path in [
        outside.path().join("present.png").display().to_string(),
        outside.path().join("missing.png").display().to_string(),
        outside
            .path()
            .join("missing/child.png")
            .display()
            .to_string(),
        "../missing-sibling/child.png".into(),
    ] {
        let result = tool.view(input(&path), &CancellationToken::new()).await;
        assert!(
            matches!(result, Err(ToolError::AccessRefused(_))),
            "{path}: {result:?}"
        );
        let result = workspace.load(&path, 1024).await;
        assert!(
            matches!(result, Err(ToolError::AccessRefused(_))),
            "{path}: {result:?}"
        );
    }
}

/// Beneath a workspace symlink to an outside directory, a missing image is
/// refused like a present one, so the error does not reveal what exists.
#[cfg(unix)]
#[tokio::test]
async fn paths_beneath_a_symlink_to_outside_are_refused_whether_or_not_they_exist() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("present.png"), b"irrelevant").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
    let tool = ViewImageTool::new(root.path()).await.unwrap();
    let workspace = ImageWorkspace::new(root.path()).await.unwrap();
    for path in [
        "link/present.png",
        "link/missing.png",
        "link/missing/child.png",
    ] {
        let result = tool.view(input(path), &CancellationToken::new()).await;
        assert!(
            matches!(result, Err(ToolError::AccessRefused(_))),
            "{path}: {result:?}"
        );
        let result = workspace.load(path, 1024).await;
        assert!(
            matches!(result, Err(ToolError::AccessRefused(_))),
            "{path}: {result:?}"
        );
    }
}

#[tokio::test]
async fn root_escape_invalid_limits_and_precancel_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("outside.png");
    std::fs::write(&path, b"irrelevant").unwrap();
    let tool = ViewImageTool::new(root.path()).await.unwrap();
    assert!(matches!(
        tool.view(input(path.to_str().unwrap()), &CancellationToken::new())
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&path, root.path().join("link.png")).unwrap();
        assert!(matches!(
            tool.view(input("link.png"), &CancellationToken::new())
                .await,
            Err(ToolError::AccessRefused(_))
        ));
    }
    for limits in [
        ViewImageLimits {
            max_pixels: 0,
            ..Default::default()
        },
        ViewImageLimits {
            max_image_bytes: usize::MAX,
            ..Default::default()
        },
        ViewImageLimits {
            max_timeout_ms: 1,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            tool.clone().with_limits(limits),
            Err(ToolError::InvalidInput(_))
        ));
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        tool.view(input("missing.png"), &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert!(matches!(
        tool.view(input(""), &CancellationToken::new()).await,
        Err(ToolError::InvalidInput(_))
    ));
}
