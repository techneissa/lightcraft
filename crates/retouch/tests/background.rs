//! Tests of the background feature (`lightcraft_retouch::background`).

mod common;

use lightcraft_retouch::background::Background;

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Background>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Background>();
}
