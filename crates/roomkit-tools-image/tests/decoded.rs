//! Full-resolution OCR pixels retain orientation and composite alpha on white.
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use roomkit_tools_core::Error;
use roomkit_tools_image::{ViewImageLimits, decode_rgb};
use std::io::Cursor;

#[test]
fn alpha_rotation_and_bounds_are_applied_before_ocr() {
    let mut pixels = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 0]));
    pixels.put_pixel(1, 0, Rgba([255, 0, 0, 255]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(pixels)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    let decoded = decode_rgb(bytes.get_ref(), &ViewImageLimits::default(), 90).unwrap();
    assert_eq!((decoded.width, decoded.height), (1, 2));
    assert_eq!(decoded.pixels, [255, 255, 255, 255, 0, 0]);
    let limits = ViewImageLimits {
        max_decode_bytes: 7,
        ..Default::default()
    };
    assert!(matches!(
        decode_rgb(bytes.get_ref(), &limits, 0),
        Err(Error::LimitExceeded(_))
    ));
}

#[test]
fn exif_precedes_explicit_rotation_without_preview_resize() {
    let mut jpeg = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(40, 20, Rgb([230, 10, 10])))
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .unwrap();
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(exif);
    bytes.extend_from_slice(&jpeg.get_ref()[2..]);
    let limits = ViewImageLimits {
        max_edge: 10,
        ..Default::default()
    };
    let decoded = decode_rgb(&bytes, &limits, 0).unwrap();
    assert_eq!((decoded.width, decoded.height), (20, 40));
    assert!(decoded.orientation_applied);
    let decoded = decode_rgb(&bytes, &limits, 90).unwrap();
    assert_eq!((decoded.width, decoded.height), (40, 20));
    assert_eq!((decoded.original_width, decoded.original_height), (40, 20));
}
