//! Tests of the hair feature (`lightcraft_retouch::hair`).

mod common;

use lightcraft_retouch::hair::Hair;

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Hair>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Hair>();
}
