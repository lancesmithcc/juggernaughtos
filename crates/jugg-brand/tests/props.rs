//! Property tests: the brand gate reads untrusted files (agent and tool output,
//! uploads). It must report, never panic.
use jugg_brand::{contrast_ratio, gate, lint_css, BrandStandards};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn gate_never_panics(text in ".{0,600}", bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let b = BrandStandards::default();
        let _ = lint_css(&b, &text);
        let _ = gate::check_svg(&b, &text, gate::Strictness::Exact);
        let _ = gate::check_svg(&b, &format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{text}</svg>"), gate::Strictness::Exact);
        let _ = gate::check_html(&b, &text);
        let _ = gate::check_raster(&b, &bytes, gate::Strictness::Exact);
        let _ = gate::check_pdf(&bytes);
        let _ = contrast_ratio(&text, "#ffffff");
    }

    #[test]
    fn svg_script_is_always_flagged(pre in "[a-z ]{0,20}", body in "[a-zA-Z0-9();]{0,40}") {
        let b = BrandStandards::default();
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{pre}<script>{body}</script></svg>");
        prop_assert!(!gate::check_svg(&b, &svg, gate::Strictness::Exact).passed);
    }
}
