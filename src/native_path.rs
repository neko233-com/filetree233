//! Lossless JSON representation of native filesystem paths.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Native {
    encoding: String,
    units: Vec<u32>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Representation {
    Text(String),
    Native(Native),
}

pub fn value(path: &Path) -> Value {
    if let Some(text) = path.to_str() {
        json!(text)
    } else {
        json!({"encoding":if cfg!(windows){"utf16"}else{"bytes"},"units":crate::tree::raw_units(path.as_os_str())})
    }
}
pub fn from_value(value: &Value) -> Result<PathBuf, String> {
    let rep: Representation =
        serde_json::from_value(value.clone()).map_err(|e| format!("invalid path: {e}"))?;
    decode(rep)
}
fn decode(rep: Representation) -> Result<PathBuf, String> {
    match rep {
        Representation::Text(text) => {
            if text.contains('\0') {
                return Err("path contains NUL".into());
            }
            Ok(PathBuf::from(text))
        }
        Representation::Native(native) => {
            if native.units.contains(&0) {
                return Err("path contains NUL".into());
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt;
                if native.encoding != "utf16" {
                    return Err("path encoding must be utf16 on Windows".into());
                }
                let units: Vec<u16> = native
                    .units
                    .into_iter()
                    .map(|unit| {
                        u16::try_from(unit).map_err(|_| "invalid UTF-16 code unit".to_string())
                    })
                    .collect::<Result<_, _>>()?;
                Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
            }
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                if native.encoding != "bytes" {
                    return Err("path encoding must be bytes on Unix".into());
                }
                let units: Vec<u8> = native
                    .units
                    .into_iter()
                    .map(|unit| u8::try_from(unit).map_err(|_| "invalid path byte".to_string()))
                    .collect::<Result<_, _>>()?;
                Ok(PathBuf::from(std::ffi::OsString::from_vec(units)))
            }
            #[cfg(not(any(unix, windows)))]
            {
                let _ = native;
                Err("native path encoding is unsupported on this platform".into())
            }
        }
    }
}
pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    value(path).serialize(serializer)
}
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    let rep = Representation::deserialize(deserializer)?;
    decode(rep).map_err(serde::de::Error::custom)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wrong_encoding_invalid_units_and_nul() {
        let wrong = if cfg!(windows) { "bytes" } else { "utf16" };
        assert!(from_value(&json!({"encoding":wrong,"units":[1]})).is_err());
        let encoding = if cfg!(windows) { "utf16" } else { "bytes" };
        assert!(from_value(&json!({"encoding":encoding,"units":[0]})).is_err());
        assert!(from_value(&json!({"encoding":encoding,"units":[65536]})).is_err());
        assert!(from_value(&json!({"encoding":encoding,"units":[1],"extra":true})).is_err());
        assert!(from_value(&json!("bad\u{0000}path")).is_err());
    }
    #[test]
    fn native_path_round_trips_through_json_without_a_filesystem() {
        #[cfg(unix)]
        let original = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0x80]))
        };
        #[cfg(windows)]
        let original = {
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_wide(&[b'x' as u16, 0xd800]))
        };
        let encoded = value(&original);
        assert!(encoded.is_object());
        assert_eq!(from_value(&encoded).unwrap(), original);
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(with = "crate::native_path")]
            path: PathBuf,
        }
        let json = serde_json::to_vec(&Wrapper {
            path: original.clone(),
        })
        .unwrap();
        let decoded: Wrapper = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded.path, original);
    }
}
