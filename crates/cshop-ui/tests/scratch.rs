//! The scratch directory the models work in.

/// The picture on its way to a model is written to the shared temporary
/// directory under a name anyone can guess. Only its owner may look inside.
#[test]
#[cfg(unix)]
fn a_scratch_directory_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = cshop_ui::vision::scratch();
    cshop_ui::vision::make_scratch(&dir).expect("made");
    let file = dir.join("source.png");
    std::fs::write(&file, b"a picture someone is editing").expect("written");

    let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "the directory is {mode:o}, so others can see into it");
    // Which also settles the file: it cannot be reached through a directory
    // nobody else may enter, whatever its own bits say.
    assert!(dir.starts_with(std::env::temp_dir()));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two runs must not collide, or one would write into the other's directory.
#[test]
fn scratch_directories_do_not_collide() {
    let a = cshop_ui::vision::scratch();
    let b = cshop_ui::vision::scratch();
    assert_ne!(a, b);
}
