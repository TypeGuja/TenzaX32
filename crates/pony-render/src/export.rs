//! Экспорт (раздел 14 ТЗ). Готово: GIF (чистый Rust, `gif` крейт) и
//! спрайт-лист (одна PNG-сетка кадров, чистый Rust, `image` крейт — тот же,
//! что уже используется для импорта текстур). MP4/WebM сознательно НЕ
//! реализованы: оба формата на практике требуют внешнего видеокодека
//! (ffmpeg/libvpx) — ни в одном чистом Rust-крейте нет полноценного,
//! проверенного энкодера этих контейнеров/кодеков, и шеллиться во внешний
//! `ffmpeg` — это зависимость от того, что может не быть установлено на
//! машине пользователя, что не вписывается в "написано на Rust, всё своё".

use crate::renderer::FrameOutput;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("no frames to export")]
    NoFrames,
    #[error("frames have mismatched dimensions: expected {expected_w}x{expected_h}, got {actual_w}x{actual_h}")]
    MismatchedDimensions { expected_w: u32, expected_h: u32, actual_w: u32, actual_h: u32 },
    #[error("failed to write to '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("GIF encoding error: {0}")]
    GifEncoding(#[from] gif::EncodingError),
    #[error("failed to save spritesheet to '{path}': {source}")]
    SpriteSheetSave {
        path: String,
        #[source]
        source: image::ImageError,
    },
}

fn check_frames(frames: &[FrameOutput]) -> Result<(u32, u32), ExportError> {
    let first = frames.first().ok_or(ExportError::NoFrames)?;
    let (width, height) = (first.width, first.height);
    for f in frames {
        if f.width != width || f.height != height {
            return Err(ExportError::MismatchedDimensions { expected_w: width, expected_h: height, actual_w: f.width, actual_h: f.height });
        }
    }
    Ok((width, height))
}

/// Закодировать последовательность отрендеренных кадров в анимированный
/// GIF. `delay_centiseconds` — задержка между кадрами в сотых долях секунды
/// (единица измерения самого формата GIF, не секунды и не мс) — например,
/// 4 ≈ 25 fps, 2 ≈ 50 fps (типичный практический потолок GIF из-за таймингов
/// большинства декодеров).
pub fn export_gif(path: &str, frames: &[FrameOutput], delay_centiseconds: u16) -> Result<(), ExportError> {
    let (width, height) = check_frames(frames)?;

    let file = std::fs::File::create(path).map_err(|source| ExportError::Io { path: path.to_string(), source })?;
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

/// Раскладка спрайт-листа, которую вернул `export_spritesheet` — нужна
/// потребителю, чтобы потом вырезать нужный кадр (например, движку,
/// который читает спрайт-лист обратно): `frame_rect(i)` даёт прямоугольник
/// i-го кадра внутри итогового PNG.
#[derive(Debug, Clone, Copy)]
pub struct SpriteSheetLayout {
    pub columns: u32,
    pub rows: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub frame_count: usize,
}

impl SpriteSheetLayout {
    /// Прямоугольник (x, y, width, height) кадра с индексом `index` внутри
    /// готового PNG. `None`, если индекс вне диапазона.
    pub fn frame_rect(&self, index: usize) -> Option<(u32, u32, u32, u32)> {
        if index >= self.frame_count {
            return None;
        }
        let col = index as u32 % self.columns;
        let row = index as u32 / self.columns;
        Some((col * self.frame_width, row * self.frame_height, self.frame_width, self.frame_height))
    }
}

/// Собрать кадры в одну PNG-сетку (спрайт-лист): `columns` кадров в ряд,
/// сколько нужно рядов — досчитывается. Пустые ячейки последнего неполного
/// ряда остаются полностью прозрачными (не мусором из памяти — `RgbaImage`
/// инициализируется нулями).
pub fn export_spritesheet(path: &str, frames: &[FrameOutput], columns: u32) -> Result<SpriteSheetLayout, ExportError> {
    let (frame_width, frame_height) = check_frames(frames)?;
    let columns = columns.max(1).min(frames.len() as u32);
    let rows = (frames.len() as u32).div_ceil(columns);

    let mut sheet = image::RgbaImage::new(frame_width * columns, frame_height * rows);
    for (i, frame) in frames.iter().enumerate() {
        let col = i as u32 % columns;
        let row = i as u32 / columns;
        let (x0, y0) = (col * frame_width, row * frame_height);
        for y in 0..frame_height {
            for x in 0..frame_width {
                let idx = ((y * frame_width + x) * 4) as usize;
                let px = image::Rgba([frame.rgba[idx], frame.rgba[idx + 1], frame.rgba[idx + 2], frame.rgba[idx + 3]]);
                sheet.put_pixel(x0 + x, y0 + y, px);
            }
        }
    }

    sheet.save(path).map_err(|source| ExportError::SpriteSheetSave { path: path.to_string(), source })?;

    Ok(SpriteSheetLayout { columns, rows, frame_width, frame_height, frame_count: frames.len() })
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
    fn gif_empty_frames_is_an_error_not_a_panic() {
        let result = export_gif("/tmp/pony_export_test_empty.gif", &[], 4);
        assert!(matches!(result, Err(ExportError::NoFrames)));
    }

    #[test]
    fn gif_mismatched_dimensions_is_an_error_not_a_panic() {
        let frames = vec![solid_frame(4, 4, [255, 0, 0, 255]), solid_frame(8, 8, [0, 255, 0, 255])];
        let result = export_gif("/tmp/pony_export_test_mismatch.gif", &frames, 4);
        assert!(matches!(result, Err(ExportError::MismatchedDimensions { .. })));
    }

    #[test]
    fn gif_writes_a_readable_gif_with_correct_frame_count() {
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

    #[test]
    fn spritesheet_empty_frames_is_an_error_not_a_panic() {
        let result = export_spritesheet("/tmp/pony_export_test_sheet_empty.png", &[], 4);
        assert!(matches!(result, Err(ExportError::NoFrames)));
    }

    #[test]
    fn spritesheet_mismatched_dimensions_is_an_error_not_a_panic() {
        let frames = vec![solid_frame(4, 4, [255, 0, 0, 255]), solid_frame(8, 8, [0, 255, 0, 255])];
        let result = export_spritesheet("/tmp/pony_export_test_sheet_mismatch.png", &frames, 2);
        assert!(matches!(result, Err(ExportError::MismatchedDimensions { .. })));
    }

    #[test]
    fn spritesheet_layout_matches_grid_and_pixels_are_in_the_right_cells() {
        let path = "/tmp/pony_export_test_sheet_roundtrip.png";
        // 4 кадра разных сплошных цветов, 2 колонки -> сетка 2x2:
        // [red, green]
        // [blue, yellow]
        let frames = vec![
            solid_frame(5, 3, [255, 0, 0, 255]),
            solid_frame(5, 3, [0, 255, 0, 255]),
            solid_frame(5, 3, [0, 0, 255, 255]),
            solid_frame(5, 3, [255, 255, 0, 255]),
        ];
        let layout = export_spritesheet(path, &frames, 2).expect("export should succeed");
        assert_eq!((layout.columns, layout.rows, layout.frame_count), (2, 2, 4));
        assert_eq!(layout.frame_rect(0), Some((0, 0, 5, 3)));
        assert_eq!(layout.frame_rect(1), Some((5, 0, 5, 3)));
        assert_eq!(layout.frame_rect(2), Some((0, 3, 5, 3)));
        assert_eq!(layout.frame_rect(3), Some((5, 3, 5, 3)));
        assert_eq!(layout.frame_rect(4), None, "индекс вне диапазона -> None");

        // Раскодируем НЕЗАВИСИМО от своего кода (`image::open`, не наши
        // внутренние структуры) и проверим, что цвет в каждой ячейке —
        // именно тот кадр, что должен был туда попасть.
        let decoded = image::open(path).expect("spritesheet should be a valid PNG").to_rgba8();
        assert_eq!(decoded.dimensions(), (10, 6));
        let expected = [
            ((1u32, 1u32), [255, 0, 0, 255]),
            ((6, 1), [0, 255, 0, 255]),
            ((1, 4), [0, 0, 255, 255]),
            ((6, 4), [255, 255, 0, 255]),
        ];
        for ((x, y), color) in expected {
            assert_eq!(decoded.get_pixel(x, y).0, color, "wrong color at ({x},{y})");
        }

        std::fs::remove_file(path).ok();
    }
}
