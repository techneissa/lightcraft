#!/usr/bin/env bash
# Runs the tests of every fork feature (FORK.md), after a merge. From any folder: fork/tests.sh
set -u
cd "$(dirname "$0")/.."
failed=()
run() { echo "==> $*"; "$@" || failed+=("$*"); }

run cargo test -p lightcraft-codecs --lib animated_gif
run cargo test -p lightcraft-filepanel
run cargo test -p lightcraft-engine --lib -- book::tests tests::book
run cargo test -p lightcraft-ui-egui --lib -- tests::book
run cargo test -p lightcraft-engine --lib -- deleting_the_first_part a_selected_brush_part the_selected_part_follows config:: layers open_path animated_gif denoise prefs quick_jobs lens_profiles camera_preview camera_profiles local_profiles smart_local smart_editing smart media presets files auto_sync quick_filters exif_edit exif_xmp sidecar::exif lens_database export::tests map location tracklog saved_locations
run cargo test -p lightcraft-lensdb
run cargo test -p lightcraft-develop
run cargo test -p lightcraft-reflections
run cargo test -p lightcraft-engine --lib -- reflections
run cargo test -p lightcraft-ui-egui --lib -- reflection
run cargo test -p lightcraft-catalog
run cargo test -p lightcraft-map
run cargo test -p lightcraft-meta gpx
run cargo test -p lightcraft-preview
run cargo test -p lightcraft-pipeline --lib -- find:: masks::tests::a_first_part_is_the_base tests_tone_sliders tests_tone tests_slider_strength tests_sharpen_mask
run cargo test -p lightcraft-ui-egui --lib -- delete_permanently delete_local quick_delete view_colour smart_editing auto_import toast render:: layers_task keybinds shortcut_editing shortcut_when shortcut tests_slider_columns edit_sections export_reveal lens_database fixed_slider_labels temp_slider dialog_fit_backup tests::keymap shortcuts:: animated_gif selection_order split_divider before_after command_palette edit_all photo_menu topbar_items titlebar loupe_stand_in the_loupe_says denoise settings profile camera_colour thumbnail dropdown_search quick_filters shortcut_backup clipping exif_edit brush_live mask_parts mask_tiles create_new_mask_tiles find_removals map
run cargo test -p lightcraft-engine --lib --features sam tests::find_removals
run cargo test -p lightcraft-raster --lib channel_clipping
run cargo test -p lightcraft-gpu --lib handles
run cargo test -p lightcraft-gpu --test it -- process_v3_sharpening
run cargo test -p lightcraft-ui-egui --lib -- delete_permanently delete_local quick_delete view_colour smart_editing auto_import toast render:: layers_task keybinds shortcut_editing shortcut_when shortcut_backup tests::keymap shortcuts:: animated_gif selection_order split_divider before_after command_palette edit_all photo_menu topbar_items titlebar loupe_stand_in the_loupe_says denoise settings profile camera_colour thumbnail dropdown_search quick_filters clipping clip_preview map

if [ ${#failed[@]} -gt 0 ]; then printf "failed: %s\n" "${failed[@]}"; exit 1; fi
echo "all fork tests passed"
