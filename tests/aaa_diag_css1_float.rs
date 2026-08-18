// SPDX-License-Identifier: GPL-3.0-or-later

//! TEMPORARY diagnostic — packet/acid1-content-box. Not part of the final
//! PR; dumps every Box fragment's rect + border + margin + background color
//! for `fixtures/css1-float-5526c.html` so the dd interior bottom-padding
//! bug (10px too much at the bottom vs Chrome's uniform 10px) can be
//! root-caused from real numbers instead of guesswork.
//! Deliberately panics so `cargo test`'s captured-output-on-failure shows
//! the dump in CI logs without needing `--nocapture` wired through.

use std::collections::HashMap;

use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, FragmentKind, Size};
use stele::style::{self, cascade};

const CSS1_FLOAT_HTML: &str = include_str!("../fixtures/css1-float-5526c.html");
const VIEWPORT_WIDTH: u32 = 800;

#[test]
fn dump_css1_float_geometry_with_margins() {
    let dom_tree = dom::parser::parse(CSS1_FLOAT_HTML);
    let author_sheets =
        style::collect_author_sheets_for_viewport(&dom_tree, VIEWPORT_WIDTH as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let root = build_box_tree(&dom_tree, &styles, &HashMap::new()).expect("root present");
    let viewport = Size { w: VIEWPORT_WIDTH as f32, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);

    let mut out = String::new();
    out.push_str("x,y,w,h,border_l,border_t,border_r,border_b,margin_l,margin_t,margin_r,margin_b,height_decl,bg\n");
    for f in &fragments {
        if let FragmentKind::Box { style } = &f.kind {
            out.push_str(&format!(
                "{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:?},{:?},{:?},{:?},{:?},{:?}\n",
                f.rect.origin.x,
                f.rect.origin.y,
                f.rect.size.w,
                f.rect.size.h,
                style.border.left.width,
                style.border.top.width,
                style.border.right.width,
                style.border.bottom.width,
                style.margin.left,
                style.margin.top,
                style.margin.right,
                style.margin.bottom,
                style.height,
                style.background_color
            ));
        }
    }
    panic!("CSS1-FLOAT GEOMETRY DUMP (with margins):\n{out}");
}
