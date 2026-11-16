//! Moving a file to the Trash, for the explorer and the Source Control
//! panel's discard.

use std::path::Path;

/// Move `path` to the Trash. On macOS through `NSFileManager`, as VS Code
/// does: the default asks the Finder to do it, which brings up "Eshvi wants
/// to control Finder".
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    let mut context = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context.delete(path).map_err(|e| format!("Could not move {} to the Trash: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_goes_to_the_trash() {
        let file = std::env::temp_dir().join(format!("eshvi-trash-test-{}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        assert_eq!(move_to_trash(&file), Ok(()));
        assert!(!file.exists());
        // A path that isn't there says so in words.
        let why = move_to_trash(&file).unwrap_err();
        assert!(why.starts_with("Could not move ") && why.contains("to the Trash"), "{why}");
    }
}
