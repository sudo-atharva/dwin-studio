//! Save every icon in an .icl as PNG: cargo run --example dump_icl -- <file.icl> <out dir>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let icl = uwin_studio::icl::Icl::parse(&std::fs::read(&a[1]).unwrap()).unwrap();
    std::fs::create_dir_all(&a[2]).unwrap();
    for (i, ic) in icl.icons.iter().enumerate() {
        if let Some(ic) = ic {
            ic.decode().unwrap().save(format!("{}/{i:03}.png", a[2])).unwrap();
        }
    }
}
