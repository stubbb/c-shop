//! What the font scan finds, and how long it takes.
use cshop_core::font::FontDb;
use std::time::Instant;

fn main() {
    let t = Instant::now();
    let db = FontDb::global();
    println!("scanned in {:?}: {} families", t.elapsed(), db.families().len());
    println!("default: {:?}", db.default_family());
    for f in db.families().iter().take(12) {
        println!("  {:30} bold:{} italic:{}", f.name, f.has_bold, f.has_italic);
    }
    let t = Instant::now();
    let font = db.load(&db.default_family(), false, false);
    println!("loading the default took {:?}, ok={}", t.elapsed(), font.is_some());
}
