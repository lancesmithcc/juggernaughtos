use image::{Rgba, RgbaImage};
use vectorverter_core::{trace_rgba, Preset, Settings, ShapeMode};

/// Transparent canvas, coral square with a transparent round hole and a teal dot inside.
fn fixture() -> RgbaImage {
    RgbaImage::from_fn(160, 160, |x, y| {
        let (dx, dy) = (x as f32 - 80.0, y as f32 - 80.0);
        let r = (dx * dx + dy * dy).sqrt();
        let in_square = (30..130).contains(&x) && (30..130).contains(&y);
        if r < 12.0 {
            Rgba([38, 142, 153, 255])
        } else if r < 30.0 || !in_square {
            Rgba([0, 0, 0, 0])
        } else {
            Rgba([250, 112, 85, 255])
        }
    })
}

#[test]
fn deterministic_output() {
    let s = Settings::default();
    let a = trace_rgba(&fixture(), &s).unwrap();
    let b = trace_rgba(&fixture(), &s).unwrap();
    assert_eq!(a.svg, b.svg);
}

#[test]
fn keeps_transparent_hole_and_inner_shape() {
    for mode in [ShapeMode::Stacked, ShapeMode::Adjoined] {
        let s = Settings { shape_mode: mode, ..Preset::Logo.settings() };
        let out = trace_rgba(&fixture(), &s).unwrap();
        assert_eq!(out.stats.colors, 2, "{:?}", out.palette);
        // The coral square needs an outer loop plus the hole (2 subpaths);
        // the teal dot is a separate shape.
        let coral = out.svg.lines().find(|l| l.contains("#fa7055")).expect("coral path");
        assert_eq!(coral.matches('M').count(), 2, "{coral}");
        assert!(out.svg.contains("#268e99"));
        assert!(out.svg.starts_with("<svg") && out.svg.trim_end().ends_with("</svg>"));
    }
}

#[test]
fn square_corners_stay_sharp() {
    let img = RgbaImage::from_fn(64, 64, |x, y| {
        if (16..48).contains(&x) && (16..48).contains(&y) { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
    });
    let out = trace_rgba(&img, &Preset::Logo.settings()).unwrap();
    let black = out.svg.lines().find(|l| l.contains("#000000")).unwrap();
    // A square should come out as four straight lines between exact corners.
    assert!(black.contains("M16 16") || black.contains("L16 16"), "{black}");
    assert!(!black.contains('C'), "square traced with curves: {black}");
}

#[test]
fn custom_palette_drops_a_colour() {
    let auto = trace_rgba(&fixture(), &Preset::Logo.settings()).unwrap();
    assert_eq!(auto.coverage.len(), auto.palette.len());
    // Remove the teal: its pixels must fall to the remaining colour.
    let keep: Vec<String> = auto.palette.iter().filter(|c| *c != "#268e99").cloned().collect();
    let s = Settings { palette: Some(keep), ..Preset::Logo.settings() };
    let out = trace_rgba(&fixture(), &s).unwrap();
    assert_eq!(out.palette, vec!["#fa7055".to_string()]);
    assert!(!out.svg.contains("#268e99"));
}

#[test]
fn tiny_square_keeps_corners_tiny_dot_stays_round() {
    let img = RgbaImage::from_fn(40, 20, |x, y| {
        let in_square = (4..10).contains(&x) && (7..13).contains(&y);
        let (dx, dy) = (x as f32 - 29.5, y as f32 - 9.5);
        let in_dot = dx * dx + dy * dy < 3.2 * 3.2;
        if in_square || in_dot { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
    });
    let out = trace_rgba(&img, &Preset::Logo.settings()).unwrap();
    let black = out.svg.lines().find(|l| l.contains("#000000")).unwrap();
    let d = black.split(" d=\"").nth(1).unwrap().trim_end_matches("\"/>");
    let subs: Vec<&str> = d.split('M').filter(|s| !s.is_empty()).collect();
    assert_eq!(subs.len(), 2, "{d}");
    // The square's subpath starts left of x = 20, the dot's right of it.
    let x0 = |s: &str| s.split(|c: char| c == ' ' || c.is_ascii_alphabetic()).next().unwrap().parse::<f32>().unwrap();
    let (square, dot) = if x0(subs[0]) < 20.0 { (subs[0], subs[1]) } else { (subs[1], subs[0]) };
    assert!(!square.contains('C'), "6px square got curves: {square}");
    assert!(dot.contains('C'), "dot traced as polygon: {dot}");
}

/// A tiny PNG whose header claims 50 000 × 50 000 pixels must be refused
/// before any pixel buffer is allocated.
#[test]
fn rejects_decompression_bomb() {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&50_000u32.to_be_bytes());
    ihdr.extend_from_slice(&50_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(&ihdr);
    png.extend_from_slice(&crc32(&ihdr).to_be_bytes());
    let err = vectorverter_core::decode(&png).unwrap_err();
    assert!(matches!(err, vectorverter_core::TraceError::TooLarge(_)), "{err}");
}

#[test]
fn odd_inputs_never_panic() {
    let s = Settings::default();
    for (w, h) in [(1, 1), (1, 7), (9, 1), (2, 2), (3, 300)] {
        let img = RgbaImage::from_fn(w, h, |x, y| if (x + y) % 2 == 0 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) });
        let _ = trace_rgba(&img, &s);
    }
    let clear = RgbaImage::new(5, 5);
    assert!(trace_rgba(&clear, &s).is_err());
    assert!(vectorverter_core::decode(b"not an image").is_err());
    assert!(vectorverter_core::decode(&[]).is_err());
    // Noise at full palette size.
    let mut seed = 7u32;
    let noise = RgbaImage::from_fn(64, 64, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        Rgba([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8, 255])
    });
    let many = Settings { colors: Some(256), ..Settings::default() };
    assert!(trace_rgba(&noise, &many).is_ok());
}

/// Off-white background with a dark ring (like the letter O): the counter is background.
fn ring_on_paper() -> RgbaImage {
    RgbaImage::from_fn(160, 160, |x, y| {
        let r = ((x as f32 - 80.0).powi(2) + (y as f32 - 80.0).powi(2)).sqrt();
        if (25.0..55.0).contains(&r) {
            Rgba([24, 22, 20, 255])
        } else {
            Rgba([248, 245, 240, 255])
        }
    })
}

#[test]
fn knockout_clears_enclosed_background_too() {
    use vectorverter_core::Background;
    let paper = |svg: &str| svg.lines().any(|l| l.contains("fill=\"#f8f5f0\"") || l.contains("fill=\"#f7f5f0\"") || l.contains("fill=\"#f8f4f0\""));
    let keep_counter = trace_rgba(&ring_on_paper(), &Settings { background: Background::Transparent, ..Preset::Logo.settings() }).unwrap();
    assert!(paper(&keep_counter.svg), "transparent mode keeps the enclosed counter: {}", keep_counter.svg);
    let knocked = trace_rgba(&ring_on_paper(), &Settings { background: Background::Knockout, ..Preset::Logo.settings() }).unwrap();
    assert!(!paper(&knocked.svg), "knockout drops it: {}", knocked.svg);
    assert_eq!(knocked.stats.colors, 1, "{:?}", knocked.palette);
    let ring = knocked.svg.lines().find(|l| l.contains("<path")).expect("ring path");
    assert_eq!(ring.matches('M').count(), 2, "ring keeps its hole: {ring}");
}
