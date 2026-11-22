use std::io;
use std::path::{Path, PathBuf};

pub const LOCALES: [(&str, &str); 2] = [("ru", include_str!("../../locales/ru.ftl")), ("en", include_str!("../../locales/en.ftl"))];
pub const ASOUND: &str = "/proc/asound";
const LIMIT: u64 = 4 * 1024 * 1024;

pub fn catalog() -> drv_i18n::Catalog {
    drv_i18n::Catalog::new(&LOCALES)
}

pub fn dumps(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for card in std::fs::read_dir(root)? {
        let card = card?.path();
        if !card.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("card")) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&card) else { continue };
        for entry in entries.flatten() {
            if entry.file_name().to_str().is_some_and(|name| name.starts_with("codec#")) {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    Ok(found)
}

pub fn read(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    io::Read::read_to_string(&mut io::Read::take(std::fs::File::open(path)?, LIMIT), &mut text)?;
    Ok(text)
}

pub fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| line.strip_prefix(name)).map(str::trim)
}

pub fn vendor(text: &str) -> Option<u32> {
    u32::from_str_radix(field(text, "Vendor Id:")?.strip_prefix("0x")?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_dumps_are_found_under_each_card() {
        let root = std::env::temp_dir().join(format!("rtkhda-asound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, file) in [("card1", "codec#0"), ("card1", "pcm0p"), ("card0", "codec#0"), ("timers", "codec#0")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(file), "Codec: Realtek ALC257\nVendor Id: 0x10ec0257\n").unwrap();
        }
        assert_eq!(dumps(&root).unwrap(), [root.join("card0/codec#0"), root.join("card1/codec#0")]);
        let text = read(&root.join("card1/codec#0")).unwrap();
        assert_eq!((field(&text, "Codec:"), vendor(&text)), (Some("Realtek ALC257"), Some(0x10ec_0257)));
        assert_eq!(vendor("Vendor Id: nonsense\n"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
