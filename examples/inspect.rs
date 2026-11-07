//! Print every control in a DWIN_SET folder with its named fields.
//! cargo run --example inspect -- <path/to/DWIN_SET>

use dwin_studio::control::{show_kind, show_params, Params, Value};
use dwin_studio::set::DwinSet;

fn fmt(p: &Params) -> String {
    p.iter()
        .map(|(n, v)| match v {
            Value::Num(x) if n.contains("vp") || *n == "sp" || n.contains("color") => format!("{n}={x:#06x}"),
            Value::Num(x) => format!("{n}={x}"),
            Value::Text(t) => format!("{n}={t:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    for d in std::env::args().skip(1) {
        let s = DwinSet::load(d.as_ref()).unwrap();
        println!("== {d}");
        for r in s.touch.iter().flat_map(|t| &t.records) {
            let base = format!(
                "page {} ({},{})-({},{}) next={:04x} pressed={:04x}",
                r.page(), r.x0, r.y0, r.x1, r.y1, r.next_page, r.pressed_page
            );
            match (r.kind(), r.params()) {
                (Some(k), Some(p)) => println!("touch {:<14} {base} {}", k.name, fmt(&p)),
                _ => println!("touch {:<14} {base} code={:04x}", "basic", r.code),
            }
        }
        for (page, recs) in s.show.iter().flat_map(|s| &s.pages) {
            for r in recs {
                match (show_kind(r), show_params(r)) {
                    (Some(k), Some(p)) => println!("show  {:<14} page {page} {}", k.name, fmt(&p)),
                    _ => println!("show  ?{:02x}           page {page} {:02x?}", r[1], r),
                }
            }
        }
        for (name, icl) in &s.icls {
            let sizes: Vec<_> = icl.icons.iter().flatten().map(|i| format!("{}x{}", i.width, i.height)).collect();
            println!("icl   {name}: {sizes:?}");
        }
    }
}
