use crate::core::{Error, Result};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use axum::http::StatusCode;
use std::sync::LazyLock;

const WIDTH: usize = 300;
const HEIGHT: usize = 450;
static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(include_bytes!(
        "../assets/source-label-font/NotoSansSC-Regular.otf"
    ))
    .expect("内置来源标签字体必须有效")
});

fn text(pixels: &mut [u8], value: &str, baseline: f32, size: f32, color: [u8; 3]) {
    let scaled = FONT.as_scaled(PxScale::from(size));
    let mut x = 24.0;
    for c in value.chars() {
        let mut glyph = scaled.scaled_glyph(c);
        let advance = scaled.h_advance(glyph.id);
        if x + advance > WIDTH as f32 - 24.0 {
            break;
        }
        glyph.position = point(x, baseline);
        if let Some(outline) = FONT.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, alpha| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= WIDTH as i32 || py >= HEIGHT as i32 {
                    return;
                }
                let offset = (py as usize * WIDTH + px as usize) * 3;
                for channel in 0..3 {
                    pixels[offset + channel] = ((1.0 - alpha) * pixels[offset + channel] as f32
                        + alpha * color[channel] as f32)
                        .round() as u8;
                }
            });
        }
        x += advance;
    }
}

fn wrap(value: &str, size: f32, lines: usize) -> Vec<String> {
    let scaled = FONT.as_scaled(PxScale::from(size));
    let mut output = vec![String::new()];
    let mut width = 0.0;
    for c in value.chars().filter(|c| !c.is_control()) {
        let advance = scaled.h_advance(scaled.glyph_id(c));
        if width + advance > WIDTH as f32 - 48.0 {
            if output.len() == lines {
                output.last_mut().unwrap().push('…');
                break;
            }
            output.push(String::new());
            width = 0.0;
        }
        output.last_mut().unwrap().push(c);
        width += advance;
    }
    output
}

pub fn render(
    source: &str,
    line: &str,
    episode: &str,
    quality: &str,
    title: &str,
) -> Result<Vec<u8>> {
    let mut pixels = vec![0; WIDTH * HEIGHT * 3];
    for y in 0..HEIGHT {
        let gain = (y * 8 / HEIGHT) as u8;
        for x in 0..WIDTH {
            let offset = (y * WIDTH + x) * 3;
            pixels[offset..offset + 3].copy_from_slice(&[14 + gain, 25 + gain, 32 + gain]);
            if !(3..WIDTH - 3).contains(&x) || !(3..HEIGHT - 3).contains(&y) {
                pixels[offset..offset + 3].copy_from_slice(&[116, 151, 89]);
            }
        }
    }
    let white = [237, 241, 235];
    let green = [195, 235, 148];
    let muted = [161, 176, 180];
    text(&mut pixels, "播放源", 36.0, 20.0, muted);
    for (index, label) in wrap(source, 36.0, 2).iter().enumerate() {
        text(&mut pixels, label, 86.0 + index as f32 * 45.0, 36.0, white);
    }
    text(&mut pixels, line, 177.0, 24.0, muted);
    text(&mut pixels, episode, 214.0, 27.0, white);
    let parts: Vec<_> = quality.split(" · ").collect();
    text(
        &mut pixels,
        parts[0],
        292.0,
        if parts.len() > 1 { 58.0 } else { 32.0 },
        green,
    );
    if let Some(dimensions) = parts.get(1) {
        text(&mut pixels, dimensions, 338.0, 27.0, white);
    }
    if parts.len() > 2 {
        text(&mut pixels, &parts[2..].join(" · "), 375.0, 21.0, muted);
    }
    for label in wrap(title, 21.0, 1) {
        text(&mut pixels, &label, 424.0, 21.0, muted);
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, WIDTH as u32, HEIGHT as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let failure = |_| {
            Error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "来源标签生成失败".to_owned(),
            )
        };
        let mut writer = encoder.write_header().map_err(failure)?;
        writer.write_image_data(&pixels).map_err(failure)?;
    }
    Ok(bytes)
}
