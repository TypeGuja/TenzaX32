//! Экспорт в GIF (раздел 14 ТЗ). PNG/MP4/WebM/спрайт-лист пока нет — GIF
//! выбран первым, потому что кодек чистый Rust (`gif` крейт), не нужен
//! внешний ffmpeg/libwebm, и результат сразу проверяем (можно открыть в
//! любом просмотрщике или тем же `gif`-декодером — см. тест ниже).

use crate::renderer::FrameOutput;

#[derive(Debug, thiserror::Error)]
pub enum GifExportError {
    #[error("no frames to export")]
    NoFrames,
    #[error("frames have mismatched dimensions: expected {expected_w}x{expected_h}, got {actual_w}x{actual_h}")]
    MismatchedDimensions { expected_w: u32, expected_h: u32, actual_w: u32, actual_h: u32 },
    #[error("failed to write GIF to '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("GIF encoding error: {0}")]
    Encoding(#[from] gif::EncodingError),
}

/// Закодировать последовательность отрендеренных кадров в анимированный
/// GIF. `delay_centiseconds` — задержка между кадрами в сотых долях секунды
/// (единица измерения самого формата GIF, не секунды и не мс) — например,
/// 4 ≈ 25 fps, 2 ≈ 50 fps (типичный практический потолок GIF из-за таймингов
/// большинства декодеров).
pub fn export_gif(path: &str, frames: &[FrameOutput], delay_centiseconds: u16) -> Result<(), GifExportError> {
    let first = frames.first().ok_or(GifExportError::NoFrames)?;
    let (width, height) = (first.width, first.height);

    for f in frames {
        if f.width != width || f.height != height {
            return Err(GifExportError::MismatchedDimensions {
                expected_w: width,
                expected_h: height,
                actual_w: f.width,
                actual_h: f.height,
            });
        }
    }

    let file = std::fs::File::create(path).map_err(|source| GifExportError::Io { path: path.to_string(), source })?;
    let mut encoder = gif::Encoder::new(file, width as u16, height as u16, &[])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;

    for frame_out in frames {
        // `from_rgba_speed` квантует RGBA8 в индексированную палитру сама
        // (NeuQuant) — не нужно вручную сводить цвета, только скорость
        // квантизации отдаём (1 = медленно/точно, 30 = быстро/грубо).
        let mut rgba = frame_out.rgba.clone();
        let mut gif_frame = gif::Frame::from_rgba_speed(width as u16, height as u16, &mut rgba, 10);
        gif_frame.delay = delay_centiseconds;
        encoder.write_frame(&gif_frame)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(width: u32, height: u32, rgba: [u8; 4]) -> FrameOutput {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        FrameOutput { width, height, rgba: data, rendered_on: "test".into() }
    }

    #[test]
    fn empty_frames_is_an_error_not_a_panic() {
        let result = export_gif("/tmp/pony_export_test_empty.gif", &[], 4);
        assert!(matches!(result, Err(GifExportError::NoFrames)));
    }

    #[test]
    fn mismatched_dimensions_is_an_error_not_a_panic() {
        let frames = vec![solid_frame(4, 4, [255, 0, 0, 255]), solid_frame(8, 8, [0, 255, 0, 255])];
        let result = export_gif("/tmp/pony_export_test_mismatch.gif", &frames, 4);
        assert!(matches!(result, Err(GifExportError::MismatchedDimensions { .. })));
    }

    #[test]
    fn writes_a_readable_gif_with_correct_frame_count() {
        let path = "/tmp/pony_export_test_roundtrip.gif";
        let frames = vec![
            solid_frame(6, 4, [255, 0, 0, 255]),
            solid_frame(6, 4, [0, 255, 0, 255]),
            solid_frame(6, 4, [0, 0, 255, 255]),
        ];
        export_gif(path, &frames, 5).expect("export should succeed");

        // Раз закодировали — тут же и раскодируем тем же независимым путём
        // (gif::Decoder, не наш код), чтобы доказать файл реально валиден,
        // а не просто "encoder ничего не упал".
        let file = std::fs::File::open(path).expect("gif file should exist");
        let mut decoder = gif::DecodeOptions::new().read_info(file).expect("gif should be decodable");
        let mut frame_count = 0;
        while decoder.read_next_frame().expect("frame should decode").is_some() {
            frame_count += 1;
        }
        assert_eq!(frame_count, 3, "should round-trip all 3 encoded frames");

        std::fs::remove_file(path).ok();
    }
}
