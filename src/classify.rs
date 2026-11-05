//! Names-only classification of tree entries so the treemap can colour by kind.
//! Heuristic and cheap: it never reads file contents and never authorizes cleanup.
use eframe::egui::Color32;
use std::ffi::OsStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Source,
    Build,
    Dependencies,
    Cache,
    VirtualEnv,
    VersionControl,
    Media,
    Documents,
    Archives,
    Logs,
    Other,
}

impl Kind {
    /// Muted base colours; the treemap lightens them with depth.
    pub fn color(self) -> Color32 {
        match self {
            Kind::Source => Color32::from_rgb(92, 138, 196),
            Kind::Build => Color32::from_rgb(86, 152, 130),
            Kind::Dependencies => Color32::from_rgb(132, 120, 184),
            Kind::Cache => Color32::from_rgb(150, 138, 108),
            Kind::VirtualEnv => Color32::from_rgb(118, 152, 94),
            Kind::VersionControl => Color32::from_rgb(182, 132, 88),
            Kind::Media => Color32::from_rgb(176, 112, 166),
            Kind::Documents => Color32::from_rgb(92, 150, 178),
            Kind::Archives => Color32::from_rgb(172, 126, 96),
            Kind::Logs => Color32::from_rgb(130, 134, 140),
            Kind::Other => Color32::from_rgb(112, 126, 144),
        }
    }
}

/// Mix `color` toward white by `level` (0 = base tone) to separate nesting depth.
pub fn shade(color: Color32, level: usize) -> Color32 {
    let step = level.min(6) as f32 / 6.0 * 0.35;
    let mix = |channel: u8| channel as f32 + (255.0 - channel as f32) * step;
    Color32::from_rgb(
        mix(color.r()).round() as u8,
        mix(color.g()).round() as u8,
        mix(color.b()).round() as u8,
    )
}

fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension.to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub fn classify(name: &OsStr, is_dir: bool) -> Kind {
    let text = name.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    let base = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    if is_dir {
        return match base {
            ".git" | ".svn" | ".hg" => Kind::VersionControl,
            "node_modules" | "vendor" | ".pnpm-store" | "site-packages" | "deps" => {
                Kind::Dependencies
            }
            ".venv" | "venv" | "virtualenv" | ".conda" | "miniconda3" | "anaconda3" => {
                Kind::VirtualEnv
            }
            "target"
            | "build"
            | "dist"
            | "out"
            | "debug"
            | "release"
            | ".gradle"
            | ".cxx"
            | "cmake-build-debug"
            | "cmake-build-release"
            | "obj"
            | "library"
            | "temp"
            | ".next"
            | ".nuxt"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".dotnet"
            | ".terragrunt-cache" => Kind::Build,
            ".cache" | "cache" | "caches" | "tmp" | ".npm" | ".nuget" | ".cargo" | "crashdumps"
            | "sentry" | "incremental" | ".fingerprint" => Kind::Cache,
            "logs" | "log" => Kind::Logs,
            "pictures" | "photos" | "music" | "videos" | "movies" | "camera" => Kind::Media,
            "documents" | "docs" | "desktop" | "downloads" => Kind::Documents,
            _ => Kind::Other,
        };
    }
    if base.starts_with('.') && extension(base).is_empty() {
        return Kind::Other;
    }
    match extension(base).as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "tif" | "tiff" | "raw"
        | "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "flac" | "mp3" | "wav" | "ogg"
        | "m4a" | "psd" | "ai" => Kind::Media,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "md" | "txt" | "rtf"
        | "epub" => Kind::Documents,
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" | "cab" | "apk"
        | "ipa" | "vhdx" | "vhd" | "ova" => Kind::Archives,
        "log" | "etl" | "dmp" | "crash" => Kind::Logs,
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "hpp" | "cpp" | "cs"
        | "java" | "kt" | "swift" | "php" | "rb" | "sh" | "ps1" | "toml" | "json" | "yaml"
        | "yml" | "xml" | "html" | "css" => Kind::Source,
        _ => Kind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directories_classify_by_name() {
        for (name, expected) in [
            ("node_modules", Kind::Dependencies),
            ("target", Kind::Build),
            (".cache", Kind::Cache),
            (".git", Kind::VersionControl),
            (".venv", Kind::VirtualEnv),
            ("random", Kind::Other),
        ] {
            assert_eq!(classify(OsStr::new(name), true), expected, "{name}");
        }
    }
    #[test]
    fn files_classify_by_extension_case_insensitively() {
        for (name, expected) in [
            ("Photo.JPG", Kind::Media),
            ("report.pdf", Kind::Documents),
            ("backup.tar.gz", Kind::Archives),
            ("main.rs", Kind::Source),
            ("app.log", Kind::Logs),
            ("LICENSE", Kind::Other),
        ] {
            assert_eq!(classify(OsStr::new(name), false), expected, "{name}");
        }
    }
    #[test]
    fn shade_lightens_toward_white_with_depth() {
        let base = Kind::Source.color();
        let deep = shade(base, 6);
        assert!(deep.r() >= base.r() && deep.g() >= base.g() && deep.b() >= base.b());
        assert_ne!(base, deep);
        assert_eq!(shade(base, 0), base);
    }
}
