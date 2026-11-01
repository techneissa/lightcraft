#!/usr/bin/env bash
# Runs the tests of every fork feature (FORK.md), after a merge. From any folder: fork/tests.sh
set -u
cd "$(dirname "$0")/.."
failed=()
run() { echo "==> $*"; "$@" || failed+=("$*"); }

run cargo test -p lightcraft-codecs --lib animated_gif
run cargo test -p lightcraft-filepanel
run cargo test -p lightcraft-engine --lib -- config:: layers open_path animated_gif denoise prefs quick_jobs lens_profiles camera_preview camera_profiles local_profiles smart_local smart_editing smart media presets files auto_sync quick_filters exif_edit exif_xmp sidecar::exif lens_database export::tests
run cargo test -p lightcraft-lensdb
run cargo test -p lightcraft-develop
run cargo test -p lightcraft-catalog
run cargo test -p lightcraft-preview
run cargo test -p lightcraft-pipeline --lib -- tests_tone_sliders tests_tone tests_slider_strength
run cargo test -p lightcraft-ui-egui --lib -- delete_permanently delete_local quick_delete view_colour smart_editing auto_import toast render:: layers_task keybinds shortcut_editing shortcut_when shortcut tests_slider_columns fixed_slider_labels temp_slider dialog_fit_backup tests::keymap shortcuts:: animated_gif selection_order split_divider before_after command_palette edit_all photo_menu topbar_items titlebar loupe_stand_in the_loupe_says denoise settings profile camera_colour thumbnail dropdown_search quick_filters shortcut_backup clipping exif_edit brush_live edit_sections export_reveal lens_database
run cargo test -p lightcraft-raster --lib channel_clipping
run cargo test -p lightcraft-pipeline --lib clip_preview
run cargo test -p lightcraft-gpu --lib handles
run cargo test -p lightcraft-gpu --test it -- process_v3_sharpening
run cargo test -p lightcraft-ui-egui --lib -- delete_permanently delete_local quick_delete view_colour smart_editing auto_import toast render:: layers_task keybinds shortcut_editing shortcut_when shortcut_backup tests::keymap shortcuts:: animated_gif selection_order split_divider before_after command_palette edit_all photo_menu topbar_items titlebar loupe_stand_in the_loupe_says denoise settings profile camera_colour thumbnail dropdown_search quick_filters clipping clip_preview

if [ ${#failed[@]} -gt 0 ]; then printf "failed: %s\n" "${failed[@]}"; exit 1; fi
echo "all fork tests passed"
