//! Tests of the skin feature (`lightcraft_retouch::skin`).

mod common;

use lightcraft_retouch::skin::Skin;

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Skin>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Skin>();
}
