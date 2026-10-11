//! Quick actions on shelf items: compress, convert and resize images, and
//! rename. Actions that produce files run the system tools (`ditto`, `sips`,
//! `cp`) in the background and add the results as new items; the original
//! files are never changed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Image formats offered by Convert Image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Heic,
    Tiff,
}

impl ImageFormat {
    pub const ALL: [Self; 4] = [Self::Png, Self::Jpeg, Self::Heic, Self::Tiff];

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Heic => "HEIC",
            Self::Tiff => "TIFF",
        }
    }

    fn sips_name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Heic => "heic",
            Self::Tiff => "tiff",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Heic => "heic",
            Self::Tiff => "tiff",
        }
    }
}

/// Longest side, in pixels, offered by Resize Image.
pub const RESIZE_SIZES: [u32; 3] = [2048, 1024, 512];

/// `report.pdf` becomes `report.pdf.zip`; several items become `Archive.zip`
/// (the names Finder's Compress uses).
pub fn archive_name(paths: &[PathBuf]) -> String {
    match paths {
        [path] => format!("{}.zip", file_name(path)),
        _ => "Archive.zip".to_owned(),
    }
}

pub fn converted_name(source: &Path, format: ImageFormat) -> String {
    format!("{}.{}", stem(source), format.extension())
}

pub fn resized_name(source: &Path, max: u32) -> String {
    match source.extension() {
        Some(ext) => format!("{} ({max} px).{}", stem(source), ext.to_string_lossy()),
        None => format!("{} ({max} px)", stem(source)),
    }
}

/// Checks a new file name typed by the user and returns it trimmed.
pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("The name can't be empty".to_owned());
    }
    if name.contains('/') || name.contains(':') {
        return Err("The name can't contain \"/\" or \":\"".to_owned());
    }
    if name.starts_with('.') {
        return Err("Names starting with \".\" are hidden".to_owned());
    }
    if name.len() > 255 {
        return Err("The name is too long".to_owned());
    }
    Ok(name.to_owned())
}

/// The renamed path in the same folder. A name typed without an extension
/// keeps the old one.
pub fn renamed_path(old: &Path, new_name: &str) -> PathBuf {
    let has_extension = Path::new(new_name).extension().is_some();
    let name = match old.extension() {
        Some(ext) if !has_extension => format!("{new_name}.{}", ext.to_string_lossy()),
        _ => new_name.to_owned(),
    };
    old.with_file_name(name)
}

/// `dir/name`, or `dir/name 2`, `dir/name 3`, ... if taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = stem(path);
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    (2..)
        .map(|n| match &ext {
            Some(ext) => dir.join(format!("{stem} {n}.{ext}")),
            None => dir.join(format!("{stem} {n}")),
        })
        .find(|candidate| !candidate.exists())
        .expect("some name is free")
}

/// `ditto` options for a plain zip, without macOS-only metadata (resource
/// forks, extended attributes, quarantine, ACLs) that would otherwise show
/// up as a `__MACOSX` folder on other systems.
const ZIP_ARGS: [&str; 6] = ["-c", "-k", "--norsrc", "--noextattr", "--noqtn", "--noacl"];

/// Zips `paths` into `out_dir`. A single item is archived as is; several are
/// gathered (as cheap APFS clones where possible) into one archive.
pub fn compress(paths: &[PathBuf], out_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let zip = unique_path(out_dir, &archive_name(paths));
    if let [path] = paths {
        // A folder is archived with its own name as the top level; a single
        // file goes in as is (with --keepParent it would sit in a folder
        // named after its parent).
        let mut ditto = Command::new("/usr/bin/ditto");
        ditto.args(ZIP_ARGS);
        if path.is_dir() {
            ditto.arg("--keepParent");
        }
        run(ditto.arg(path).arg(&zip))?;
        return Ok(vec![zip]);
    }
    let staging = out_dir.join(".staging");
    fs::create_dir_all(&staging)?;
    let result = (|| {
        for path in paths {
            let target = unique_path(&staging, &file_name(path));
            // `-c` clones on APFS (instant, no extra space); across volumes it
            // fails, so fall back to a plain copy.
            let cloned = run(Command::new("/bin/cp").arg("-cR").arg(path).arg(&target));
            if cloned.is_err() {
                run(Command::new("/bin/cp").arg("-R").arg(path).arg(&target))?;
            }
        }
        run(Command::new("/usr/bin/ditto")
            .args(ZIP_ARGS)
            .arg(&staging)
            .arg(&zip))
    })();
    let _ = fs::remove_dir_all(&staging);
    result.map(|()| vec![zip])
}

/// Converts each image into `out_dir`.
pub fn convert(
    images: &[PathBuf],
    format: ImageFormat,
    out_dir: &Path,
) -> io::Result<Vec<PathBuf>> {
    images
        .iter()
        .map(|image| {
            let out = unique_path(out_dir, &converted_name(image, format));
            run(Command::new("/usr/bin/sips")
                .args(["-s", "format", format.sips_name()])
                .arg(image)
                .arg("--out")
                .arg(&out))?;
            Ok(out)
        })
        .collect()
}

/// Scales each image down so its longest side is `max` pixels. Images that
/// are already that small are skipped; it is an error if all of them are.
pub fn resize(images: &[PathBuf], max: u32, out_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut outputs = Vec::new();
    for image in images {
        if longest_side(image)? <= max {
            continue;
        }
        let out = unique_path(out_dir, &resized_name(image, max));
        run(Command::new("/usr/bin/sips")
            .args(["-Z", &max.to_string()])
            .arg(image)
            .arg("--out")
            .arg(&out))?;
        outputs.push(out);
    }
    if outputs.is_empty() {
        return Err(io::Error::other(format!("already {max} px or smaller")));
    }
    Ok(outputs)
}

fn longest_side(image: &Path) -> io::Result<u32> {
    let output = Command::new("/usr/bin/sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(image)
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    parse_longest_side(&text).ok_or_else(|| io::Error::other("could not read the image size"))
}

/// Reads `pixelWidth: 1920` / `pixelHeight: 1080` lines from `sips -g`.
fn parse_longest_side(sips_output: &str) -> Option<u32> {
    sips_output
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            matches!(key, "pixelWidth" | "pixelHeight").then(|| value.trim().parse::<u32>().ok())?
        })
        .max()
}

fn run(command: &mut Command) -> io::Result<()> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(io::Error::other(if message.is_empty() {
            format!("{command:?} failed")
        } else {
            message
        }))
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(archive_name(&["/a/Report.pdf".into()]), "Report.pdf.zip");
        assert_eq!(archive_name(&["/a/x".into(), "/b/y".into()]), "Archive.zip");
        assert_eq!(
            converted_name(Path::new("/a/Photo.heic"), ImageFormat::Jpeg),
            "Photo.jpg"
        );
        assert_eq!(
            resized_name(Path::new("/a/Photo.png"), 1024),
            "Photo (1024 px).png"
        );
        assert_eq!(resized_name(Path::new("/a/Photo"), 512), "Photo (512 px)");
    }

    #[test]
    fn renaming() {
        assert_eq!(validate_name("  New name  ").unwrap(), "New name");
        assert!(validate_name("").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a:b").is_err());
        assert!(validate_name(".hidden").is_err());
        assert_eq!(
            renamed_path(Path::new("/i/Text.txt"), "Notes"),
            Path::new("/i/Notes.txt")
        );
        assert_eq!(
            renamed_path(Path::new("/i/Text.txt"), "Notes.md"),
            Path::new("/i/Notes.md")
        );
        assert_eq!(
            renamed_path(Path::new("/i/Folder"), "Stuff"),
            Path::new("/i/Stuff")
        );
    }

    #[test]
    fn unique_paths() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(tmp.path(), "a.txt"), tmp.path().join("a.txt"));
        fs::write(tmp.path().join("a.txt"), "").unwrap();
        fs::write(tmp.path().join("a 2.txt"), "").unwrap();
        assert_eq!(unique_path(tmp.path(), "a.txt"), tmp.path().join("a 3.txt"));
        fs::create_dir(tmp.path().join("dir")).unwrap();
        assert_eq!(unique_path(tmp.path(), "dir"), tmp.path().join("dir 2"));
    }

    #[test]
    fn sips_sizes() {
        let out = "/x/Photo.png\n  pixelWidth: 1920\n  pixelHeight: 1080\n";
        assert_eq!(parse_longest_side(out), Some(1920));
        assert_eq!(parse_longest_side("nothing"), None);
    }

    #[test]
    fn compresses_one_and_several_items() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, out) = (
            tmp.path().join("a.txt"),
            tmp.path().join("dir"),
            tmp.path().join("out"),
        );
        fs::write(&a, "alpha").unwrap();
        fs::create_dir_all(b.join("inner")).unwrap();
        fs::write(b.join("inner/b.txt"), "beta").unwrap();
        fs::create_dir_all(&out).unwrap();

        let single = compress(std::slice::from_ref(&a), &out).unwrap();
        assert_eq!(single, vec![out.join("a.txt.zip")]);
        let several = compress(&[a.clone(), b.clone()], &out).unwrap();
        assert_eq!(several, vec![out.join("Archive.zip")]);
        assert!(!out.join(".staging").exists(), "staging is cleaned up");

        let names = |zip: &Path| {
            let listing = Command::new("/usr/bin/zipinfo")
                .arg("-1")
                .arg(zip)
                .output()
                .unwrap();
            String::from_utf8_lossy(&listing.stdout).into_owned()
        };
        assert_eq!(
            names(&single[0]).trim(),
            "a.txt",
            "a single file sits at the top level"
        );
        let folder = compress(std::slice::from_ref(&b), &out).unwrap();
        assert!(
            names(&folder[0]).contains("dir/inner/b.txt"),
            "a folder keeps its name"
        );
        let all = names(&several[0]);
        assert!(
            all.contains("a.txt") && all.contains("dir/inner/b.txt"),
            "{all}"
        );
        assert!(!all.contains("__MACOSX") && !names(&single[0]).contains("__MACOSX"));
        assert!(a.exists() && b.exists(), "originals untouched");
    }

    #[test]
    fn converts_and_resizes_images() {
        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("Pic.png");
        // A 40 x 20 image made with sips from a built-in TIFF.
        let made = Command::new("/usr/bin/sips")
            .args(["-s", "format", "png", "-z", "20", "40"])
            .arg("/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericDocumentIcon.icns")
            .arg("--out")
            .arg(&png)
            .output()
            .unwrap();
        assert!(made.status.success());
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();

        let jpeg = convert(std::slice::from_ref(&png), ImageFormat::Jpeg, &out).unwrap();
        assert_eq!(jpeg, vec![out.join("Pic.jpg")]);
        assert_eq!(longest_side(&jpeg[0]).unwrap(), 40);

        let small = resize(std::slice::from_ref(&png), 20, &out).unwrap();
        assert_eq!(small, vec![out.join("Pic (20 px).png")]);
        assert_eq!(longest_side(&small[0]).unwrap(), 20);
        assert!(
            resize(std::slice::from_ref(&png), 512, &out).is_err(),
            "no upscaling"
        );
        assert_eq!(longest_side(&png).unwrap(), 40, "original untouched");
    }
}
