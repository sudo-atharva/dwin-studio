//! Parse every real DGUS output folder next to this project and re-emit it.
//! Must be byte-identical to what DGUS wrote.

use dwin_studio::set::DwinSet;
use std::path::PathBuf;

fn sample_dirs() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut dirs: Vec<PathBuf> = (1..=5)
        .map(|n| root.join(format!("DWIN_Training_Demos-master/Demo_{n}/HMI/DWIN_SET")))
        .collect();
    dirs.push(root.join("DWIN2_Display-main/DWIN_Display_Interface/DWIN_SET"));
    dirs.retain(|d| d.is_dir());
    assert!(!dirs.is_empty(), "no sample DWIN_SET folders found");
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
