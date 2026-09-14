#[path = "../src/storage_dialog.rs"]
mod storage_dialog;
use std::path::PathBuf;

#[test]
fn cancelling_the_native_picker_does_not_select_a_default_or_change_a_path() {
    assert_eq!(storage_dialog::selected_directory(None).unwrap(), None);
}

#[test]
fn the_native_picker_preserves_spaces_and_unicode_in_absolute_locations() {
    let path = if cfg!(windows) {
        PathBuf::from("C:\\Work SSD\\ação")
    } else {
        PathBuf::from("/Volumes/Work SSD/ação")
    };
    assert_eq!(
        storage_dialog::selected_directory(Some(path.clone())).unwrap(),
        Some(path.to_str().unwrap().to_owned())
    );
}

#[test]
fn relative_picker_results_are_rejected_instead_of_resolved_against_an_arbitrary_working_directory()
{
    assert!(storage_dialog::selected_directory(Some(PathBuf::from("storage"))).is_err());
}

#[cfg(unix)]
#[test]
fn non_utf8_picker_results_do_not_silently_change_to_a_different_path() {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"/storage/\xff".to_vec()));
    assert!(storage_dialog::selected_directory(Some(path)).is_err());
}
