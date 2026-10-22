//! Where the sketch's files are. A running sketch's folder is the current
//! directory, and data/ is inside it (where load_image and friends look).

use std::path::PathBuf;

fn sketch_dir() -> PathBuf { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }

/// sketchPath(): the sketch folder, as an absolute path.
pub fn sketch_path() -> String { sketch_dir().to_string_lossy().into_owned() }
/// sketchPath(name): a file in the sketch folder (absolute paths are kept).
pub fn sketch_path_of(name: &str) -> String { sketch_file(name).to_string_lossy().into_owned() }
pub fn sketch_file(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() { p } else { sketch_dir().join(p) }
}
/// dataPath(name): a file in the sketch's data folder.
pub fn data_path(name: &str) -> String { data_file(name).to_string_lossy().into_owned() }
pub fn data_file(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() { p } else { sketch_dir().join("data").join(p) }
}
/// savePath(name): sketch_path_of(name), with its folders made if missing.
pub fn save_path(name: &str) -> String {
    let p = sketch_file(name);
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    p.to_string_lossy().into_owned()
}
/// listPaths(dir): absolute paths of what is in a folder (relative to the
/// sketch folder), sorted. Empty if it is not a folder.
pub fn list_paths(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(sketch_file(dir)) else { return Vec::new() };
    let mut v: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.path().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}
/// listFiles(dir): the same as list_paths.
pub fn list_files(dir: &str) -> Vec<String> { list_paths(dir) }

#[inline] pub fn sketchPath() -> String { sketch_path() }
#[inline] pub fn dataPath(name: &str) -> String { data_path(name) }
#[inline] pub fn savePath(name: &str) -> String { save_path(name) }
#[inline] pub fn sketchFile(name: &str) -> PathBuf { sketch_file(name) }
#[inline] pub fn dataFile(name: &str) -> PathBuf { data_file(name) }
#[inline] pub fn listPaths(dir: &str) -> Vec<String> { list_paths(dir) }
#[inline] pub fn listFiles(dir: &str) -> Vec<String> { list_files(dir) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths() {
        let here = sketch_path();
        assert!(std::path::Path::new(&here).is_absolute());
        assert_eq!(sketch_path_of("a.txt"), sketch_dir().join("a.txt").to_string_lossy());
        assert_eq!(data_file("x.png"), sketch_dir().join("data").join("x.png"));
        let abs = std::env::temp_dir().join("abs.txt");
        assert_eq!(data_file(abs.to_str().unwrap()), abs);

        let root = std::env::temp_dir().join(format!("rm_paths_{}", std::process::id()));
        let f = save_path(root.join("sub/deep/out.txt").to_str().unwrap());
        assert!(root.join("sub/deep").is_dir());
        std::fs::write(&f, "x").unwrap();
        std::fs::write(root.join("sub/b.txt"), "y").unwrap();
        let list = list_files(root.join("sub").to_str().unwrap());
        assert_eq!(list.len(), 2);
        assert!(list[0].ends_with("b.txt") && list[1].ends_with("deep"));
        assert!(list_paths("/no/such/folder/here").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
