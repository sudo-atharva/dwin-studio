use dwin_studio::set::DwinSet;
fn main() {
    for d in std::env::args().skip(1) {
        let s = DwinSet::load(d.as_ref()).unwrap();
        let t = s.touch.as_ref().map_or(0, |t| t.records.len());
        let sh: usize = s.show.as_ref().map_or(0, |s| s.pages.values().map(Vec::len).sum());
        let icl: Vec<_> = s.icls.iter().map(|(n, i)| format!("{n}:{}icons/align{}", i.icons.iter().flatten().count(), i.align)).collect();
        println!("{d}\n  touch {t}, show {sh}, icl {icl:?}, raw {:?}", s.raw.keys().collect::<Vec<_>>());
    }
}
