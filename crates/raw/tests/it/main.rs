//! The integration tests, one module per file, linked into one test binary.
//! `tests/sr2_heap.rs` stays a binary of its own: it installs dhat as the global allocator.

/// A [`lightcraft_raw::RawInfo`] as a header probe can know it. A full CR3 decode finds how far the samples
/// really saturate above Canon's stated white level and adds that headroom to `baseline_exposure` (#3805); a
/// probe reads no samples, so for a CR3 that one field is left out of a probe-against-decode comparison. Every
/// other format, and every other field, must match.
fn probe_comparable(mut info: lightcraft_raw::RawInfo) -> lightcraft_raw::RawInfo {
    if info.format == lightcraft_raw::RawFormat::Cr3 {
        info.color.baseline_exposure = 0.0;
    }
    info
}

mod body_corpus;
mod canon_pentax_corpus;
mod corpus;
mod coverage_sweep_corpus;
mod cr3_corpus;
mod cr3_variants;
mod crw_corpus;
mod crx_properties;
mod dng;
mod lossy_dng;
mod nrw_be16;
mod olympus_samsung_corpus;
mod rendered_dng;
mod robust;
mod sony_leica_corpus;
mod sony_lens_inventory;
mod sparse_corpus;
mod vendor_robust;
