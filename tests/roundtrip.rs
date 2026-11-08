//! Parse every real DGUS output folder next to this project and re-emit it.
//! Must be byte-identical to what DGUS wrote.

use uwin_studio::set::DwinSet;
use std::path::PathBuf;

fn sample_dirs() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut dirs: Vec<PathBuf> = (1..=5)
        .map(|n| root.join(format!("DWIN_Training_Demos-master/Demo_{n}/HMI/DWIN_SET")))
        .collect();
    dirs.push(root.join("DWIN2_Display-main/DWIN_Display_Interface/DWIN_SET"));
    dirs.retain(|d| d.is_dir());
    if dirs.is_empty() {
        eprintln!("no DGUS sample folders next to the project: skipping (see README, Tests)");
    }
    dirs
}

#[test]
fn byte_identical_roundtrip() {
    let mut failures = Vec::new();
    for dir in sample_dirs() {
        let set = DwinSet::load(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for (name, bytes) in set.files() {
            let orig = std::fs::read(dir.join(&name)).unwrap();
            if orig != bytes {
                let at = orig.iter().zip(&bytes).position(|(a, b)| a != b);
                failures.push(format!(
                    "{}/{name}: len {} vs {}, first diff at {at:?}",
                    dir.display(),
                    orig.len(),
                    bytes.len()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Every icon DGUS wrote decodes, and re-packing its JPEG gives the same entry bytes.
#[test]
fn sample_icons_decode_and_repack() {
    for dir in sample_dirs() {
        let set = DwinSet::load(&dir).unwrap();
        for (name, icl) in &set.icls {
            for (i, icon) in icl.icons.iter().enumerate() {
                let Some(icon) = icon else { continue };
                let img = icon.decode().unwrap_or_else(|e| panic!("{name}[{i}]: {e}"));
                assert_eq!((img.width(), img.height()), (icon.width as u32, icon.height as u32));
                let jpeg = icon.to_jpeg().unwrap();
                let re = uwin_studio::picture::pack_jpeg(&jpeg, icon.width, icon.height).unwrap();
                assert!(re == *icon, "{}/{name}[{i}]: repack differs", dir.display());
            }
        }
    }
}

/// Every record of a known kind must survive decode -> encode through named fields only.
/// Proves the field tables cover every non-zero byte DGUS writes.
#[test]
fn typed_controls_roundtrip() {
    use uwin_studio::control::{show_kind, show_params, show_record};
    use std::collections::BTreeMap;

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for dir in sample_dirs() {
        let set = DwinSet::load(&dir).unwrap();
        for r in set.touch.iter().flat_map(|t| &t.records) {
            let name = match r.kind() {
                Some(k) => {
                    let mut copy = r.clone();
                    copy.set_params(k, &r.params().unwrap());
                    if copy != *r {
                        failures.push(format!("{}: touch {} {:02x?}", dir.display(), k.name, r.ext));
                    }
                    k.name.to_string()
                }
                None if r.code >> 8 >= 0xFD => format!("UNTYPED touch {:04x}", r.code),
                None => "basic_touch".into(),
            };
            *seen.entry(name).or_default() += 1;
        }
        for r in set.show.iter().flat_map(|s| s.pages.values().flatten()) {
            let name = match show_kind(r) {
                Some(k) => {
                    if show_record(k, &show_params(r).unwrap()) != *r {
                        failures.push(format!("{}: show {} {:02x?}", dir.display(), k.name, r));
                    }
                    k.name.to_string()
                }
                None => format!("UNTYPED show {:02x}", r[1]),
            };
            *seen.entry(name).or_default() += 1;
        }
    }
    eprintln!("controls seen: {seen:#?}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    assert!(!seen.keys().any(|k| k.starts_with("UNTYPED")), "untyped kinds: {seen:?}");
}

/// Both shipped examples load and build with no warnings (VP clashes, overlaps, bad targets).
#[test]
fn examples_build_clean() {
    for f in ["examples/demo.uwin", "examples/showcase/showcase.uwin"] {
        let p: uwin_studio::project::Project = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let dir = std::path::Path::new(f).parent().unwrap();
        let out = uwin_studio::build::build(&p, dir).unwrap();
        assert!(out.warnings.is_empty(), "{f}: {:?}", out.warnings);
    }
}
