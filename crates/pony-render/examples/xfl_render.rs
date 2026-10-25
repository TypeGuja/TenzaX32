//! Отрендерить кадр символа из .fla/.xfl в PNG (проверка импорта Adobe
//! Animate без GUI):
//!
//! ```text
//! cargo run -p pony-render --example xfl_render -- file.fla            # список символов
//! cargo run -p pony-render --example xfl_render -- file.fla "~RD_Character" 0 out.png [scale]
//! ```
//! Имя символа `-` — главная временная шкала.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: xfl_render <file.fla> [symbol|-] [frame] [out.png] [scale]");
        std::process::exit(2);
    };
    let t = std::time::Instant::now();
    let doc = match pony_core::xfl::XflDocument::open(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("parsed in {:.2}s: {} symbols, stage {}x{}", t.elapsed().as_secs_f32(), doc.symbols.len(), doc.width, doc.height);
    for w in &doc.warnings {
        eprintln!("warning: {w}");
    }
    let Some(symbol) = args.get(1) else {
        for name in doc.symbol_names() {
            let tl = &doc.symbols[name];
            println!("{name}  ({} frames, {} layers)", tl.frame_count(), tl.layers.len());
        }
        return;
    };
    let symbol = (symbol != "-").then_some(symbol.as_str());
    let frame: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let out = args.get(3).cloned().unwrap_or_else(|| "xfl_frame.png".into());
    let scale: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    if let Some(tl) = doc.timeline(symbol) {
        eprintln!("frames: {}, labels: {:?}", tl.frame_count(), tl.frame_labels());
    }
    if std::env::var("XFL_PARTS").is_ok() {
        let opts = pony_core::xfl::ImportOptions { expand_containers: true, ..Default::default() };
        for p in doc.import_parts(symbol, frame, &opts).map(|r| r.parts).unwrap_or_default() {
            eprintln!("part {:<40} bbox {:?} origin {:?}", p.id, p.rendered.bbox, p.origin);
        }
    }
    let Some(r) = doc.render_svg(symbol, frame) else {
        eprintln!("nothing to render");
        std::process::exit(1);
    };
    eprintln!("svg: {} KB, bbox {:?}, unsupported elements: {}", r.svg.len() / 1024, r.bbox, r.unsupported);
    let tree = usvg::Tree::from_str(&r.svg, &usvg::Options::default()).expect("svg parse");
    let size = tree.size();
    let (w, h) = (((size.width() * scale).ceil() as u32).max(1), ((size.height() * scale).ceil() as u32).max(1));
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.save_png(&out).expect("save png");
    eprintln!("wrote {out} ({w}x{h}) in {:.2}s total", t.elapsed().as_secs_f32());
}
