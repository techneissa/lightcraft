//! Tests of the glare feature (`lightcraft_retouch::glare`).

mod common;

use lightcraft_retouch::glare::Glare;

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Glare>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Glare>();
}
