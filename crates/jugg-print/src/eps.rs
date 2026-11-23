//! SVG → EPS (Encapsulated PostScript) for print shops and older design tools.
//!
//! The SVG is normalised by usvg (absolute paths, resolved transforms, text as
//! outlines), then every visible path is written as PostScript fills and strokes.
//! Gradients and patterns fall back to their first colour: logos are flat colour.

use crate::{tree, PrintError, Result};
use svg2pdf::usvg::{self, tiny_skia_path::PathSegment, Node, Paint};

fn colour(p: &Paint) -> (u8, u8, u8) {
    let c = match p {
        Paint::Color(c) => *c,
        Paint::LinearGradient(g) => g.stops().first().map(|s| s.color()).unwrap_or(usvg::Color::black()),
        Paint::RadialGradient(g) => g.stops().first().map(|s| s.color()).unwrap_or(usvg::Color::black()),
        Paint::Pattern(_) => usvg::Color::black(),
    };
    (c.red, c.green, c.blue)
}

fn n(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn path_ops(out: &mut String, path: &usvg::Path, height: f32) {
    let t = path.abs_transform();
    let pt = |p: usvg::tiny_skia_path::Point| {
        let mut q = p;
        t.map_point(&mut q);
        // PostScript's origin is bottom-left.
        format!("{} {}", n(q.x), n(height - q.y))
    };
    let mut last = usvg::tiny_skia_path::Point::zero();
    out.push_str("newpath\n");
    for seg in path.data().segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                out.push_str(&format!("{} m\n", pt(p)));
                last = p;
            }
            PathSegment::LineTo(p) => {
                out.push_str(&format!("{} l\n", pt(p)));
                last = p;
            }
            PathSegment::QuadTo(c, p) => {
                // Quadratic → cubic: control points two thirds of the way to c.
                let c1 = usvg::tiny_skia_path::Point::from_xy(last.x + 2.0 / 3.0 * (c.x - last.x), last.y + 2.0 / 3.0 * (c.y - last.y));
                let c2 = usvg::tiny_skia_path::Point::from_xy(p.x + 2.0 / 3.0 * (c.x - p.x), p.y + 2.0 / 3.0 * (c.y - p.y));
                out.push_str(&format!("{} {} {} c\n", pt(c1), pt(c2), pt(p)));
                last = p;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                out.push_str(&format!("{} {} {} c\n", pt(c1), pt(c2), pt(p)));
                last = p;
            }
            PathSegment::Close => out.push_str("h\n"),
        }
    }
}

fn walk(out: &mut String, g: &usvg::Group, height: f32) {
    for node in g.children() {
        match node {
            Node::Group(g) => walk(out, g, height),
            Node::Text(t) => walk(out, t.flattened(), height),
            Node::Image(_) => {}
            Node::Path(p) => {
                if !p.is_visible() {
                    continue;
                }
                if let Some(f) = p.fill() {
                    let (r, gr, b) = colour(f.paint());
                    path_ops(out, p, height);
                    out.push_str(&format!("{} {} {} rg {}\n", n(r as f32 / 255.0), n(gr as f32 / 255.0), n(b as f32 / 255.0), if f.rule() == usvg::FillRule::EvenOdd { "eofill" } else { "fill" }));
                }
                if let Some(s) = p.stroke() {
                    let (r, gr, b) = colour(s.paint());
                    let t = p.abs_transform();
                    let scale = ((t.sx * t.sx + t.ky * t.ky).sqrt() + (t.kx * t.kx + t.sy * t.sy).sqrt()) / 2.0;
                    path_ops(out, p, height);
                    out.push_str(&format!("{} {} {} rg {} setlinewidth stroke\n", n(r as f32 / 255.0), n(gr as f32 / 255.0), n(b as f32 / 255.0), n(s.width().get() * scale)));
                }
            }
        }
    }
}

/// One EPS file at the SVG's own size (1 SVG unit = 1 point).
pub fn eps_from_svg(svg: &str, title: &str) -> Result<Vec<u8>> {
    let t = tree(svg)?;
    let (w, h) = (t.size().width(), t.size().height());
    if !(w > 0.0 && h > 0.0) {
        return Err(PrintError::Render("empty size".into()));
    }
    let title: String = title.chars().filter(|c| !c.is_control() && *c != '(' && *c != ')' && *c != '\\').take(120).collect();
    let mut out = format!(
        "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 {} {}\n%%HiResBoundingBox: 0 0 {} {}\n%%Title: ({title})\n%%Creator: (Juggernaught OS)\n%%Pages: 1\n%%EndComments\n\
         %%BeginProlog\n/m {{moveto}} bind def /l {{lineto}} bind def /c {{curveto}} bind def /h {{closepath}} bind def /rg {{setrgbcolor}} bind def\n%%EndProlog\n%%Page: 1 1\ngsave\n",
        w.ceil() as i64, h.ceil() as i64, n(w), n(h)
    );
    walk(&mut out, t.root(), h);
    out.push_str("grestore\nshowpage\n%%EOF\n");
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_flipped_filled_paths() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50"><rect x="10" y="10" width="20" height="10" fill="#ff0000"/><path d="M0 0 Q50 50 100 0" fill="none" stroke="#0000ff" stroke-width="2"/></svg>"##;
        let eps = String::from_utf8(eps_from_svg(svg, "Test (logo)").unwrap()).unwrap();
        assert!(eps.starts_with("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 50"));
        assert!(eps.contains("%%Title: (Test logo)"));
        assert!(eps.contains("10 40 m"), "y is flipped: {eps}");
        assert!(eps.contains("1 0 0 rg fill"));
        assert!(eps.contains("0 0 1 rg 2 setlinewidth stroke"));
        assert!(eps.contains(" c\n"), "quadratic became a cubic");
        assert!(eps.trim_end().ends_with("%%EOF"));
    }
}
