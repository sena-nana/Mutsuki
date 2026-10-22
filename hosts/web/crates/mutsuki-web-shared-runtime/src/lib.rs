//! Shared browser runtime embedded from `hosts/web/packages/web-runtime/browser/shared`.
//!
//! The shell serves these modules once under content-addressed names and maps their bare
//! specifiers through an inline import map, so extensions externalize `vue`, `vue-router`,
//! `pinia`, `@mutsuki/web-sdk`, `@mutsuki/ui` and `@mutsuki/web-shell` instead of bundling
//! private copies. Nothing here runs Node or Vite: the files are prebuilt and committed.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use sha2::{Digest, Sha256};

/// Directory, relative to the document root, holding every shared file.
pub const SHARED_DIR: &str = "shared";

/// Hex digits of the SHA-256 kept in a file name; the server treats `.<hex≥8>` stems as
/// immutable.
const NAME_HASH_LEN: usize = 16;

macro_rules! shared_file {
    ($file:literal) => {
        (
            $file,
            include_bytes!(concat!(
                "../../../packages/web-runtime/browser/shared/",
                $file
            ))
            .as_slice(),
        )
    };
}

/// `(specifier, built file)` for every import-mapped module, in import-map order.
const MODULES: [(&str, (&str, &[u8])); 6] = [
    ("vue", shared_file!("vue.js")),
    ("vue-router", shared_file!("vue-router.js")),
    ("pinia", shared_file!("pinia.js")),
    ("@mutsuki/web-sdk", shared_file!("web-sdk.js")),
    ("@mutsuki/ui", shared_file!("ui.js")),
    ("@mutsuki/web-shell", shared_file!("web-shell.js")),
];

/// The design-system stylesheet the shell links once for every page.
const STYLESHEET: (&str, &[u8]) = shared_file!("ui.css");

/// One embedded file under its content-addressed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFile {
    /// Source name in `browser/shared`, e.g. `vue.js`.
    pub source_name: &'static str,
    /// Served path relative to the document root, e.g. `shared/vue.0123456789abcdef.js`.
    pub path: String,
    /// `sha256:<hex>` of `bytes`, the same form extension manifests use.
    pub content_hash: String,
    pub bytes: &'static [u8],
}

/// The embedded shared runtime.
#[derive(Debug)]
pub struct SharedRuntime {
    modules: Vec<(&'static str, SharedFile)>,
    stylesheet: SharedFile,
    import_map_json: String,
}

static RUNTIME: LazyLock<SharedRuntime> = LazyLock::new(SharedRuntime::embedded);

/// The process-wide shared runtime; hashing happens once.
#[must_use]
pub fn shared_runtime() -> &'static SharedRuntime {
    &RUNTIME
}

impl SharedRuntime {
    fn embedded() -> Self {
        let modules = MODULES
            .iter()
            .map(|(specifier, (name, bytes))| (*specifier, shared_file(name, bytes)))
            .collect::<Vec<_>>();
        let stylesheet = shared_file(STYLESHEET.0, STYLESHEET.1);
        let imports = modules
            .iter()
            .map(|(specifier, file)| {
                (
                    (*specifier).to_string(),
                    serde_json::Value::String(format!("./{}", file.path)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let import_map_json = serde_json::json!({ "imports": imports }).to_string();
        Self {
            modules,
            stylesheet,
            import_map_json,
        }
    }

    /// Every file to serve: the modules, then the stylesheet.
    pub fn files(&self) -> impl Iterator<Item = &SharedFile> {
        self.modules
            .iter()
            .map(|(_, file)| file)
            .chain(std::iter::once(&self.stylesheet))
    }

    /// Bare specifiers the import map resolves.
    pub fn specifiers(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.modules.iter().map(|(specifier, _)| *specifier)
    }

    /// Served path of one mapped module.
    #[must_use]
    pub fn module_path(&self, specifier: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|(candidate, _)| *candidate == specifier)
            .map(|(_, file)| file.path.as_str())
    }

    /// Served path of the design-system stylesheet.
    #[must_use]
    pub fn stylesheet_path(&self) -> &str {
        &self.stylesheet.path
    }

    /// Canonical import-map JSON with document-relative URLs. This exact text is what the
    /// CSP hash covers, so it must be inlined byte-for-byte.
    #[must_use]
    pub fn import_map_json(&self) -> &str {
        &self.import_map_json
    }

    /// The inline `<script type="importmap">` element for the console document.
    #[must_use]
    pub fn import_map_script(&self) -> String {
        format!(
            "<script type=\"importmap\">{}</script>",
            self.import_map_json
        )
    }

    /// CSP `script-src` source allowing exactly the inline import map.
    #[must_use]
    pub fn csp_script_hash(&self) -> String {
        csp_hash(self.import_map_json.as_bytes())
    }

    /// Writes every shared file below `root` at its served path.
    ///
    /// # Errors
    ///
    /// Returns the first I/O error from creating the directory or writing a file.
    pub fn write_to(&self, root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join(SHARED_DIR))?;
        for file in self.files() {
            std::fs::write(root.join(&file.path), file.bytes)?;
        }
        Ok(())
    }
}

fn shared_file(name: &'static str, bytes: &'static [u8]) -> SharedFile {
    let digest = Sha256::digest(bytes);
    let hex = hex(&digest);
    let (stem, extension) = name
        .rsplit_once('.')
        .expect("shared files have an extension");
    SharedFile {
        source_name: name,
        path: format!("{SHARED_DIR}/{stem}.{}.{extension}", &hex[..NAME_HASH_LEN]),
        content_hash: format!("sha256:{hex}"),
        bytes,
    }
}

/// `'sha256-<base64>'` CSP source for one inline script body.
#[must_use]
pub fn csp_hash(script_body: &[u8]) -> String {
    format!("'sha256-{}'", base64(&Sha256::digest(script_body)))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Standard padded base64 (RFC 4648), the encoding CSP hash sources require.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0u32, |acc, (index, byte)| {
            acc | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(triple >> (18 - 6 * index) & 0x3f) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn csp_hash_matches_known_digest() {
        // sha256("alert('Hello, world.');"), the example in the CSP specification.
        assert_eq!(
            csp_hash(b"alert('Hello, world.');"),
            "'sha256-qznLcsROx4GACP2dm0UCKCzCG+HiZ1guq6ZZDob/Tng='"
        );
    }

    #[test]
    fn embedded_files_match_the_built_directory() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/web-runtime/browser/shared");
        let mut built = std::fs::read_dir(&dir)
            .expect("browser/shared is built and committed")
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        built.sort();
        let mut embedded = shared_runtime()
            .files()
            .map(|file| file.source_name.to_string())
            .collect::<Vec<_>>();
        embedded.sort();
        assert_eq!(
            built, embedded,
            "embed list drifted from web-runtime output"
        );
    }

    #[test]
    fn import_map_maps_every_specifier_to_its_content_addressed_file() {
        let runtime = shared_runtime();
        let map: serde_json::Value = serde_json::from_str(runtime.import_map_json()).unwrap();
        let imports = map["imports"].as_object().unwrap();
        assert_eq!(imports.len(), MODULES.len());
        for specifier in runtime.specifiers() {
            let path = runtime.module_path(specifier).unwrap();
            assert_eq!(imports[specifier], format!("./{path}"));
            let file = runtime.files().find(|file| file.path == path).unwrap();
            let stem = Path::new(path).file_stem().unwrap().to_str().unwrap();
            let (_, hash) = stem.rsplit_once('.').unwrap();
            let digest = file.content_hash.strip_prefix("sha256:").unwrap();
            assert_eq!(hash, &digest[..NAME_HASH_LEN]);
        }
        assert!(runtime.stylesheet_path().starts_with("shared/ui."));
        assert_eq!(
            runtime.csp_script_hash(),
            csp_hash(runtime.import_map_json().as_bytes())
        );
        assert!(
            runtime
                .import_map_script()
                .contains(runtime.import_map_json())
        );
    }

    #[test]
    fn write_to_places_files_at_served_paths() {
        let dir =
            std::env::temp_dir().join(format!("mutsuki-shared-runtime-{}", std::process::id()));
        shared_runtime().write_to(&dir).unwrap();
        for file in shared_runtime().files() {
            assert_eq!(std::fs::read(dir.join(&file.path)).unwrap(), file.bytes);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
