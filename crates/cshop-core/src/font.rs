//! The font database.
//!
//! Fonts come from the system rather than being bundled: an image editor is
//! expected to offer whatever the user has installed, and 80 MB of font data
//! does not belong in the binary.
//!
//! Reading and parsing every installed face costs a few hundred milliseconds,
//! which is far too long to spend on the frame where someone picks the Type
//! tool. The scan therefore runs once on a background thread — started at
//! launch — and anything that needs the list waits for it.

use ab_glyph::FontVec;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// One installed face: where it lives and what it looks like.
#[derive(Debug, Clone)]
struct FaceEntry {
    path: PathBuf,
    /// Index within a font collection (`.ttc`); zero for a plain file.
    index: u32,
    bold: bool,
    italic: bool,
}

/// A family and the faces it contains.
#[derive(Debug, Clone)]
pub struct Family {
    pub name: String,
    /// True when the family ships a real bold or italic, rather than needing
    /// one synthesised.
    pub has_bold: bool,
    pub has_italic: bool,
    faces: Vec<FaceEntry>,
}

/// A family and the style asked of it: what a loaded face is cached under.
type FaceKey = (String, bool, bool);

pub struct FontDb {
    families: Vec<Family>,
    /// Faces already read off disk, keyed by family and style.
    loaded: Mutex<HashMap<FaceKey, Option<Arc<FontVec>>>>,
}

/// Where fonts are installed. Missing directories are skipped.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
    }
    dirs
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out, depth + 1);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("ttf" | "otf" | "ttc")
        ) {
            out.push(path);
        }
    }
}

/// Read the family and subfamily out of a face's `name` table.
fn face_names(face: &ttf_parser::Face<'_>) -> Option<(String, String)> {
    // Name IDs 16/17 are the typographic family and subfamily, which keep
    // "Light" and "Semibold" as styles of one family instead of splitting them
    // into families of their own. Fall back to 1/2 when they are absent.
    let pick = |id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .filter(|n| n.name_id == id && n.is_unicode())
            .find_map(|n| n.to_string())
            .filter(|s| !s.is_empty())
    };
    let family = pick(16).or_else(|| pick(1))?;
    let subfamily = pick(17).or_else(|| pick(2)).unwrap_or_else(|| "Regular".into());
    Some((family, subfamily))
}

impl FontDb {
    /// Walk the font directories. Slow; call from a background thread.
    fn scan() -> FontDb {
        let mut files = Vec::new();
        for dir in font_dirs() {
            collect_files(&dir, &mut files, 0);
        }
        files.sort();

        let mut by_family: HashMap<String, Family> = HashMap::new();
        for path in files {
            let Ok(data) = std::fs::read(&path) else { continue };
            let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
            for index in 0..count {
                let Ok(face) = ttf_parser::Face::parse(&data, index) else { continue };
                let Some((family, subfamily)) = face_names(&face) else { continue };

                // Trust the face's own flags over the subfamily string, which
                // is inconsistent across foundries.
                let italic = face.is_italic() || face.is_oblique();
                let bold = face.is_bold() || subfamily.to_ascii_lowercase().contains("bold");

                let entry = FaceEntry { path: path.clone(), index, bold, italic };
                let slot = by_family.entry(family.clone()).or_insert_with(|| Family {
                    name: family.clone(),
                    has_bold: false,
                    has_italic: false,
                    faces: Vec::new(),
                });
                slot.has_bold |= bold;
                slot.has_italic |= italic;
                // One face per style; the first file wins, and the sorted scan
                // makes that deterministic.
                if !slot.faces.iter().any(|f| f.bold == bold && f.italic == italic) {
                    slot.faces.push(entry);
                }
            }
        }

        let mut families: Vec<Family> = by_family.into_values().collect();
        families.sort_by_key(|f| f.name.to_lowercase());
        FontDb { families, loaded: Mutex::new(HashMap::new()) }
    }

    /// The database, waiting for the background scan if it is still running.
    pub fn global() -> &'static FontDb {
        static DB: OnceLock<FontDb> = OnceLock::new();
        DB.get_or_init(FontDb::scan)
    }

    /// Start the scan early, so picking the Type tool does not stall.
    pub fn warm_up() {
        std::thread::spawn(|| {
            let db = FontDb::global();
            log::info!("font scan found {} families", db.families.len());
        });
    }

    pub fn families(&self) -> &[Family] {
        &self.families
    }

    pub fn family(&self, name: &str) -> Option<&Family> {
        self.families.iter().find(|f| f.name == name)
    }

    /// The family a new text layer starts with.
    pub fn default_family(&self) -> String {
        // A predictable, widely installed sans first; otherwise whatever is
        // there, so the tool still works on a minimal system.
        for want in ["DejaVu Sans", "Liberation Sans", "Noto Sans", "FreeSans", "Ubuntu", "Arial"] {
            if self.family(want).is_some() {
                return want.to_string();
            }
        }
        self.families.first().map(|f| f.name.clone()).unwrap_or_default()
    }

    /// Load a face, falling back to the nearest style the family has.
    ///
    /// A family with no italic still answers a request for one — the caller
    /// gets the upright face and slants it itself, which is what a word
    /// processor's "faux italic" is.
    pub fn load(&self, family: &str, bold: bool, italic: bool) -> Option<Arc<FontVec>> {
        let key = (family.to_string(), bold, italic);
        if let Some(hit) = self.loaded.lock().ok()?.get(&key) {
            return hit.clone();
        }

        let font = self.load_uncached(family, bold, italic);
        if let Ok(mut cache) = self.loaded.lock() {
            cache.insert(key, font.clone());
        }
        font
    }

    fn load_uncached(&self, family: &str, bold: bool, italic: bool) -> Option<Arc<FontVec>> {
        let family = self.family(family)?;
        // Exact style, then one that drops italic, then one that drops bold,
        // then anything at all.
        let pick = family
            .faces
            .iter()
            .find(|f| f.bold == bold && f.italic == italic)
            .or_else(|| family.faces.iter().find(|f| f.bold == bold))
            .or_else(|| family.faces.iter().find(|f| f.italic == italic))
            .or_else(|| family.faces.first())?;

        let data = std::fs::read(&pick.path).ok()?;
        FontVec::try_from_vec_and_index(data, pick.index).ok().map(Arc::new)
    }

    /// Whether the exact style exists, so the caller knows to synthesise.
    pub fn has_exact(&self, family: &str, bold: bool, italic: bool) -> bool {
        self.family(family)
            .is_some_and(|f| f.faces.iter().any(|e| e.bold == bold && e.italic == italic))
    }
}
