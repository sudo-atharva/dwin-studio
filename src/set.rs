//! A whole `DWIN_SET/` folder: parsed touch/show/icl files plus everything else kept raw
//! (fonts `.HZK/.dzk`, `22_Config.bin` VP init memory, `T5LCFG.CFG`, images).

use crate::{icl::Icl, show::ShowFile, touch::TouchFile};
use std::collections::BTreeMap;
use std::path::Path;

pub const TOUCH: &str = "13TouchFile.bin";
pub const SHOW: &str = "14ShowFile.bin";

#[derive(Debug, Clone, Default)]
pub struct DwinSet {
    pub touch: Option<TouchFile>,
    pub show: Option<ShowFile>,
    /// File name (e.g. "32.icl") -> library.
    pub icls: BTreeMap<String, Icl>,
    /// Every other file, byte-for-byte.
    pub raw: BTreeMap<String, Vec<u8>>,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

impl DwinSet {
    pub fn load(dir: &Path) -> Res<Self> {
        let mut set = Self::default();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let data = std::fs::read(&path)?;
            let ctx = |e| format!("{name}: {e}");
            if name.eq_ignore_ascii_case(TOUCH) {
                set.touch = Some(TouchFile::parse(&data).map_err(ctx)?);
            } else if name.eq_ignore_ascii_case(SHOW) {
                set.show = Some(ShowFile::parse(&data).map_err(ctx)?);
            } else if name.to_ascii_lowercase().ends_with(".icl") {
                let icl = Icl::parse(&data).map_err(ctx)?;
                set.icls.insert(name, icl);
            } else {
                set.raw.insert(name, data);
            }
        }
        Ok(set)
    }

    /// Every output file as (name, bytes).
    pub fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<_> = self.raw.clone().into_iter().collect();
        if let Some(t) = &self.touch {
            out.push((TOUCH.into(), t.to_bytes()));
        }
        if let Some(s) = &self.show {
            out.push((SHOW.into(), s.to_bytes()));
        }
        out.extend(self.icls.iter().map(|(n, i)| (n.clone(), i.to_bytes())));
        out
    }

    /// Write all files into `dir`. Screen files left there by an earlier build (pictures,
    /// libraries, fonts, configs we didn't write this time) are removed first: the screen would
    /// load them from the SD card too. Other files are left alone.
    pub fn write(&self, dir: &Path) -> Res<()> {
        std::fs::create_dir_all(dir)?;
        let files = self.files();
        let ours: std::collections::HashSet<String> = files.iter().map(|(n, _)| n.to_lowercase()).collect();
        for e in std::fs::read_dir(dir)?.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let screen_file = ["icl", "jpg", "bin", "hzk", "dzk", "cfg"].iter().any(|x| name.ends_with(&format!(".{x}")));
            if screen_file && !ours.contains(&name) && e.path().is_file() {
                std::fs::remove_file(e.path())?;
            }
        }
        for (name, data) in files {
            std::fs::write(dir.join(name), data)?;
        }
        Ok(())
    }
}
